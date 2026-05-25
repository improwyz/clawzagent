import { Badge, statusVariant } from '../shared/Badge';
import { approveAction, rejectAction, type Approval } from '../../lib/api';
import { useQueryClient } from '@tanstack/react-query';

interface GovernancePanelProps {
  approvals: Approval[];
  loading?: boolean;
}

export function GovernancePanel({ approvals, loading = false }: GovernancePanelProps) {
  const qc = useQueryClient();

  const handleApprove = async (id: string) => {
    await approveAction(id);
    qc.invalidateQueries({ queryKey: ['governance'] });
  };

  const handleReject = async (id: string) => {
    await rejectAction(id);
    qc.invalidateQueries({ queryKey: ['governance'] });
  };

  if (loading) {
    return (
      <div className="space-y-2">
        {Array.from({ length: 3 }).map((_, i) => (
          <div key={i} className="h-14 bg-zinc-800 rounded-lg animate-pulse" />
        ))}
      </div>
    );
  }

  const pending = approvals.filter((a) => a.status === 'pending');
  const recent = approvals.filter((a) => a.status !== 'pending').slice(0, 5);

  return (
    <div className="space-y-3">
      {pending.length > 0 && (
        <div>
          <div className="text-xs font-medium text-zinc-400 uppercase tracking-wider mb-2">
            Pending ({pending.length})
          </div>
          <div className="space-y-2">
            {pending.map((a) => (
              <div key={a.id} className="p-3 rounded-lg bg-zinc-800 border border-yellow-500/20">
                <div className="flex items-start justify-between gap-2 mb-2">
                  <div>
                    <div className="text-zinc-100 text-sm font-medium">{a.agent_name}</div>
                    <div className="text-zinc-400 text-xs">{a.action}</div>
                    {a.details && (
                      <div className="text-zinc-500 text-xs mt-0.5 font-mono">{a.details}</div>
                    )}
                  </div>
                  <span className="text-zinc-500 text-xs whitespace-nowrap">
                    {new Date(a.requested_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                  </span>
                </div>
                <div className="flex gap-2">
                  <button
                    onClick={() => handleApprove(a.id)}
                    className="px-2 py-1 text-xs rounded bg-green-600/20 text-green-400 hover:bg-green-600/30 transition-colors"
                  >
                    Approve
                  </button>
                  <button
                    onClick={() => handleReject(a.id)}
                    className="px-2 py-1 text-xs rounded bg-red-600/20 text-red-400 hover:bg-red-600/30 transition-colors"
                  >
                    Reject
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {recent.length > 0 && (
        <div>
          <div className="text-xs font-medium text-zinc-400 uppercase tracking-wider mb-2">
            Recent
          </div>
          <div className="space-y-1.5">
            {recent.map((a) => (
              <div key={a.id} className="flex items-center justify-between p-2.5 rounded-lg bg-zinc-800/50">
                <div>
                  <span className="text-zinc-200 text-xs font-medium">{a.agent_name}</span>
                  <span className="text-zinc-500 text-xs ml-1.5">{a.action}</span>
                </div>
                <Badge variant={statusVariant(a.status)}>{a.status}</Badge>
              </div>
            ))}
          </div>
        </div>
      )}

      {approvals.length === 0 && (
        <div className="py-4 text-center text-zinc-500 text-sm">No approval requests</div>
      )}
    </div>
  );
}
