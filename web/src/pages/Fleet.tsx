import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { StatCard } from '../components/widgets/StatCard';
import { FleetTable } from '../components/widgets/FleetTable';
import { Badge, statusVariant } from '../components/shared/Badge';
import { Modal } from '../components/shared/Modal';
import { EmptyState } from '../components/shared/EmptyState';
import { fetchFleet, deployAgent, fetchAgents, type FleetNode, type Deployment } from '../lib/api';

const PROVIDERS = ['anthropic', 'openai', 'google', 'local'];

function MeshTopology({ nodes }: { nodes: FleetNode[] }) {
  const W = 400;
  const H = 200;
  const cx = W / 2;
  const cy = H / 2;
  const r = Math.min(cx, cy) - 30;

  const positions = nodes.map((_, i) => ({
    x: cx + r * Math.cos((2 * Math.PI * i) / nodes.length - Math.PI / 2),
    y: cy + r * Math.sin((2 * Math.PI * i) / nodes.length - Math.PI / 2),
  }));

  return (
    <svg viewBox={`0 0 ${W} ${H}`} className="w-full" style={{ maxHeight: '180px' }}>
      {/* Connections between all nodes */}
      {nodes.map((_, i) =>
        nodes.slice(i + 1).map((__, j) => (
          <line
            key={`${i}-${i + 1 + j}`}
            x1={positions[i].x}
            y1={positions[i].y}
            x2={positions[i + 1 + j].x}
            y2={positions[i + 1 + j].y}
            stroke="#27272a"
            strokeWidth="1"
          />
        )),
      )}
      {/* Nodes */}
      {nodes.map((node, i) => (
        <g key={node.id}>
          <circle
            cx={positions[i].x}
            cy={positions[i].y}
            r={10}
            fill={
              node.status === 'online' ? '#16a34a' :
              node.status === 'busy' ? '#ca8a04' : '#dc2626'
            }
            opacity={0.8}
          />
          <text
            x={positions[i].x}
            y={positions[i].y + 20}
            textAnchor="middle"
            fill="#71717a"
            fontSize="9"
          >
            {node.hostname.split('.')[0].slice(0, 10)}
          </text>
        </g>
      ))}
      {nodes.length === 0 && (
        <text x={cx} y={cy} textAnchor="middle" fill="#52525b" fontSize="12">
          No nodes
        </text>
      )}
    </svg>
  );
}

function DeployModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient();
  const { data: fleet } = useQuery({ queryKey: ['fleet'], queryFn: fetchFleet });
  const { data: agents = [] } = useQuery({ queryKey: ['agents'], queryFn: fetchAgents });
  const [agentId, setAgentId] = useState('');
  const [nodeId, setNodeId] = useState('');
  const [provider, setProvider] = useState(PROVIDERS[0]);
  const [error, setError] = useState('');

  const mutation = useMutation({
    mutationFn: () => deployAgent(agentId, nodeId, provider),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['fleet'] });
      onClose();
    },
    onError: (e) => setError(String(e)),
  });

  const submit = () => {
    if (!agentId || !nodeId) { setError('Select agent and node'); return; }
    setError('');
    mutation.mutate();
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Deploy Agent"
      size="md"
      footer={
        <>
          <button onClick={onClose} className="px-3 py-1.5 rounded-lg text-sm bg-zinc-700 text-zinc-300 hover:bg-zinc-600">Cancel</button>
          <button onClick={submit} disabled={mutation.isPending} className="px-3 py-1.5 rounded-lg text-sm bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50">
            {mutation.isPending ? 'Deploying...' : 'Deploy'}
          </button>
        </>
      }
    >
      <div className="space-y-3">
        {error && <div className="px-3 py-2 bg-red-900/30 border border-red-700 rounded text-red-400 text-sm">{error}</div>}
        <div>
          <label className="block text-zinc-400 text-xs mb-1">Agent *</label>
          <select className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500"
            value={agentId} onChange={(e) => setAgentId(e.target.value)}>
            <option value="">Select agent...</option>
            {agents.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
          </select>
        </div>
        <div>
          <label className="block text-zinc-400 text-xs mb-1">Target Node *</label>
          <select className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500"
            value={nodeId} onChange={(e) => setNodeId(e.target.value)}>
            <option value="">Select node...</option>
            {(fleet?.nodes ?? []).map((n) => <option key={n.id} value={n.id}>{n.hostname} ({n.region})</option>)}
          </select>
        </div>
        <div>
          <label className="block text-zinc-400 text-xs mb-1">Provider</label>
          <select className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500"
            value={provider} onChange={(e) => setProvider(e.target.value)}>
            {PROVIDERS.map((p) => <option key={p} value={p}>{p}</option>)}
          </select>
        </div>
      </div>
    </Modal>
  );
}

export function Fleet() {
  const [deployOpen, setDeployOpen] = useState(false);
  const [tab, setTab] = useState<'nodes' | 'mesh' | 'deployments'>('nodes');

  const { data, isLoading } = useQuery({
    queryKey: ['fleet'],
    queryFn: fetchFleet,
    refetchInterval: 15_000,
  });

  const nodes = data?.nodes ?? [];
  const deployments = data?.deployments ?? [];

  const online = nodes.filter((n) => n.status === 'online').length;
  const offline = nodes.filter((n) => n.status === 'offline').length;
  const avgCpu = nodes.length
    ? Math.round(nodes.reduce((s, n) => s + n.cpu_pct, 0) / nodes.length)
    : 0;

  return (
    <div className="flex flex-col h-full overflow-y-auto p-4 gap-4">
      <div className="flex items-center justify-between">
        <h2 className="text-zinc-100 font-semibold">Fleet</h2>
        <button
          onClick={() => setDeployOpen(true)}
          className="px-3 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500"
        >
          + Deploy Agent
        </button>
      </div>

      {/* Stats */}
      <div className="grid grid-cols-4 gap-3">
        <StatCard label="Total Nodes" value={nodes.length} loading={isLoading} />
        <StatCard label="Online" value={online} loading={isLoading} variant="success" />
        <StatCard label="Offline" value={offline} loading={isLoading} variant={offline > 0 ? 'error' : 'default'} />
        <StatCard label="Avg CPU" value={avgCpu} unit="%" loading={isLoading} variant={avgCpu > 80 ? 'warning' : 'default'} />
      </div>

      {/* Tabs */}
      <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800 overflow-hidden flex-1">
        <div className="flex border-b border-zinc-800">
          {(['nodes', 'mesh', 'deployments'] as const).map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={`px-4 py-2.5 text-sm font-medium transition-colors capitalize ${
                tab === t ? 'text-blue-400 border-b-2 border-blue-500' : 'text-zinc-500 hover:text-zinc-300'
              }`}
            >
              {t}
            </button>
          ))}
        </div>

        <div className="p-4">
          {tab === 'nodes' && (
            nodes.length === 0 && !isLoading ? (
              <EmptyState icon="🖥️" title="No fleet nodes" description="Nodes will appear when workers connect." />
            ) : (
              <FleetTable nodes={nodes} loading={isLoading} />
            )
          )}

          {tab === 'mesh' && (
            <div>
              <p className="text-zinc-500 text-xs mb-4">
                Mesh topology — {nodes.length} node{nodes.length !== 1 ? 's' : ''}
              </p>
              <MeshTopology nodes={nodes} />
              {nodes.length > 0 && (
                <div className="mt-4 flex gap-4 text-xs text-zinc-500">
                  <span className="flex items-center gap-1"><span className="w-2 h-2 rounded-full bg-green-600 inline-block" /> Online</span>
                  <span className="flex items-center gap-1"><span className="w-2 h-2 rounded-full bg-yellow-600 inline-block" /> Busy</span>
                  <span className="flex items-center gap-1"><span className="w-2 h-2 rounded-full bg-red-600 inline-block" /> Offline</span>
                </div>
              )}
            </div>
          )}

          {tab === 'deployments' && (
            deployments.length === 0 && !isLoading ? (
              <EmptyState icon="🚀" title="No deployments yet" description="Deploy an agent to a fleet node." />
            ) : (
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-zinc-800">
                    <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Agent</th>
                    <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Node</th>
                    <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Provider</th>
                    <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Status</th>
                    <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Deployed</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-zinc-800">
                  {deployments.map((d: Deployment) => (
                    <tr key={d.id} className="hover:bg-zinc-800/50">
                      <td className="px-3 py-2.5 text-zinc-200">{d.agent_name}</td>
                      <td className="px-3 py-2.5 text-zinc-400 font-mono text-xs">{d.node_name}</td>
                      <td className="px-3 py-2.5 text-zinc-400 text-xs">{d.provider}</td>
                      <td className="px-3 py-2.5">
                        <Badge variant={statusVariant(d.status)}>{d.status}</Badge>
                      </td>
                      <td className="px-3 py-2.5 text-zinc-500 text-xs font-mono">
                        {new Date(d.deployed_at).toLocaleString()}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )
          )}
        </div>
      </div>

      <DeployModal open={deployOpen} onClose={() => setDeployOpen(false)} />
    </div>
  );
}
