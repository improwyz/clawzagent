import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { StatCard } from '../components/widgets/StatCard';
import { Badge, statusVariant } from '../components/shared/Badge';
import { EmptyState } from '../components/shared/EmptyState';
import { Modal } from '../components/shared/Modal';
import { fetchTools, toggleTool, executeTool, type Tool, type McpServer, type DockerTool } from '../lib/api';

const CATEGORY_ICONS: Record<string, string> = {
  web: '🌐',
  code: '⚙',
  database: '⬡',
  file: '📄',
  api: '↔',
  email: '@',
  calendar: '📅',
  search: '🔍',
  mcp: '◈',
};

type Tab = 'catalog' | 'docker' | 'mcp' | 'execute';

function ToolCard({ tool, onToggle }: { tool: Tool; onToggle: (id: string, enabled: boolean) => void }) {
  const icon = CATEGORY_ICONS[tool.category?.toLowerCase()] ?? '⚙';
  return (
    <div className="widget-3d p-3 rounded-xl bg-zinc-900 border border-zinc-800 flex flex-col gap-2">
      <div className="flex items-start justify-between gap-2">
        <div className="flex items-center gap-2">
          <div className="w-8 h-8 rounded-lg bg-zinc-800 flex items-center justify-center text-sm flex-shrink-0">
            {icon}
          </div>
          <div>
            <div className="text-zinc-100 text-sm font-medium">{tool.name}</div>
            <div className="text-zinc-500 text-xs">{tool.category}</div>
          </div>
        </div>
        <button
          onClick={() => onToggle(tool.id, !tool.enabled)}
          className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors flex-shrink-0 ${
            tool.enabled ? 'bg-blue-600' : 'bg-zinc-700'
          }`}
        >
          <span
            className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition-transform ${
              tool.enabled ? 'translate-x-4' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>
      <p className="text-zinc-500 text-xs leading-relaxed">{tool.description}</p>
    </div>
  );
}

function ExecutePanel({ tools }: { tools: Tool[] }) {
  const [toolId, setToolId] = useState('');
  const [argsText, setArgsText] = useState('{}');
  const [result, setResult] = useState<string | null>(null);
  const [argError, setArgError] = useState('');
  const [duration, setDuration] = useState<number | null>(null);

  const mutation = useMutation({
    mutationFn: ({ id, args }: { id: string; args: Record<string, unknown> }) =>
      executeTool(id, args),
    onSuccess: (data) => {
      setResult(JSON.stringify(data.result, null, 2));
      setDuration(data.duration_ms);
    },
    onError: (e) => setResult(`Error: ${String(e)}`),
  });

  const execute = () => {
    setArgError('');
    let args: Record<string, unknown>;
    try {
      args = JSON.parse(argsText) as Record<string, unknown>;
    } catch {
      setArgError('Invalid JSON');
      return;
    }
    if (!toolId) { setArgError('Select a tool'); return; }
    mutation.mutate({ id: toolId, args });
  };

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className="block text-zinc-400 text-xs mb-1">Tool</label>
          <select
            className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500"
            value={toolId}
            onChange={(e) => setToolId(e.target.value)}
          >
            <option value="">Select tool...</option>
            {tools.filter((t) => t.enabled).map((t) => (
              <option key={t.id} value={t.id}>{t.name}</option>
            ))}
          </select>
        </div>
        <div />
      </div>
      <div>
        <label className="block text-zinc-400 text-xs mb-1">Arguments (JSON)</label>
        <textarea
          className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500 font-mono resize-none h-24"
          value={argsText}
          onChange={(e) => setArgsText(e.target.value)}
        />
        {argError && <div className="text-red-400 text-xs mt-1">{argError}</div>}
      </div>
      <button
        onClick={execute}
        disabled={mutation.isPending}
        className="px-4 py-2 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500 disabled:opacity-50"
      >
        {mutation.isPending ? 'Executing...' : 'Execute'}
      </button>
      {result !== null && (
        <div>
          <div className="flex items-center justify-between mb-1">
            <label className="text-zinc-400 text-xs">Result</label>
            {duration !== null && (
              <span className="text-zinc-500 text-xs font-mono">{duration}ms</span>
            )}
          </div>
          <pre className="bg-zinc-800 border border-zinc-700 rounded-lg p-3 text-zinc-200 text-xs font-mono overflow-auto max-h-48">
            {result}
          </pre>
        </div>
      )}
    </div>
  );
}

export function Tools() {
  const qc = useQueryClient();
  const [tab, setTab] = useState<Tab>('catalog');
  const [mcpModal, setMcpModal] = useState(false);

  const { data, isLoading } = useQuery({
    queryKey: ['tools'],
    queryFn: fetchTools,
    refetchInterval: 30_000,
  });

  const toggleMut = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) => toggleTool(id, enabled),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['tools'] }),
  });

  const tools = data?.tools ?? [];
  const enabled = tools.filter((t) => t.enabled).length;

  return (
    <div className="flex flex-col h-full overflow-y-auto p-4 gap-4">
      <div className="flex items-center justify-between">
        <h2 className="text-zinc-100 font-semibold">Tools</h2>
      </div>

      {/* Stats */}
      <div className="grid grid-cols-4 gap-3">
        <StatCard label="Total Tools" value={tools.length} loading={isLoading} />
        <StatCard label="Enabled" value={enabled} loading={isLoading} variant="success" />
        <StatCard label="MCP Servers" value={data?.mcp_servers.length ?? 0} loading={isLoading} />
        <StatCard label="Docker Tools" value={data?.docker_tools.length ?? 0} loading={isLoading} />
      </div>

      {/* Tabs */}
      <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800 overflow-hidden flex-1">
        <div className="flex border-b border-zinc-800">
          {(['catalog', 'docker', 'mcp', 'execute'] as Tab[]).map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={`px-4 py-2.5 text-sm font-medium transition-colors capitalize ${
                tab === t ? 'text-blue-400 border-b-2 border-blue-500' : 'text-zinc-500 hover:text-zinc-300'
              }`}
            >
              {t === 'mcp' ? 'MCP Servers' : t}
            </button>
          ))}
        </div>

        <div className="p-4">
          {tab === 'catalog' && (
            tools.length === 0 && !isLoading ? (
              <EmptyState icon="🔧" title="No tools registered" description="Tools are loaded from the provider configuration." />
            ) : (
              <div className="grid grid-cols-3 gap-3">
                {isLoading
                  ? Array.from({ length: 9 }).map((_, i) => (
                    <div key={i} className="h-20 bg-zinc-800 rounded-xl animate-pulse" />
                  ))
                  : tools.map((tool) => (
                    <ToolCard
                      key={tool.id}
                      tool={tool}
                      onToggle={(id, en) => toggleMut.mutate({ id, enabled: en })}
                    />
                  ))}
              </div>
            )
          )}

          {tab === 'docker' && (
            (data?.docker_tools ?? []).length === 0 && !isLoading ? (
              <EmptyState icon="🐳" title="No Docker tools" description="Deploy a tool container from the library." />
            ) : (
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-zinc-800">
                    <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Name</th>
                    <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Image</th>
                    <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Status</th>
                    <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Port</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-zinc-800">
                  {(data?.docker_tools ?? []).map((dt: DockerTool) => (
                    <tr key={dt.id} className="hover:bg-zinc-800/50">
                      <td className="px-3 py-2.5 text-zinc-200">{dt.name}</td>
                      <td className="px-3 py-2.5 text-zinc-400 text-xs font-mono">{dt.image}</td>
                      <td className="px-3 py-2.5">
                        <Badge variant={statusVariant(dt.status)}>{dt.status}</Badge>
                      </td>
                      <td className="px-3 py-2.5 text-zinc-400 text-xs font-mono">
                        {dt.port ?? '—'}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )
          )}

          {tab === 'mcp' && (
            <div>
              <div className="flex justify-end mb-3">
                <button
                  onClick={() => setMcpModal(true)}
                  className="px-3 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500"
                >
                  + Add Server
                </button>
              </div>
              {(data?.mcp_servers ?? []).length === 0 && !isLoading ? (
                <EmptyState icon="◈" title="No MCP servers" description="Connect an MCP server to extend tool capabilities." />
              ) : (
                <div className="space-y-2">
                  {(data?.mcp_servers ?? []).map((srv: McpServer) => (
                    <div key={srv.id} className="flex items-center gap-3 p-3 rounded-lg bg-zinc-800">
                      <div className="w-8 h-8 rounded-lg bg-zinc-700 flex items-center justify-center text-sm">◈</div>
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2">
                          <span className="text-zinc-100 text-sm font-medium">{srv.name}</span>
                          <Badge variant={statusVariant(srv.status)}>{srv.status}</Badge>
                        </div>
                        <div className="text-zinc-500 text-xs font-mono">{srv.url}</div>
                      </div>
                      <div className="text-zinc-400 text-xs font-mono">{srv.tools_count} tools</div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}

          {tab === 'execute' && (
            <ExecutePanel tools={tools} />
          )}
        </div>
      </div>

      <Modal open={mcpModal} onClose={() => setMcpModal(false)} title="Add MCP Server" size="md">
        <div className="space-y-3">
          <div>
            <label className="block text-zinc-400 text-xs mb-1">Server Name</label>
            <input className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500" placeholder="my-mcp-server" />
          </div>
          <div>
            <label className="block text-zinc-400 text-xs mb-1">URL</label>
            <input className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500 font-mono" placeholder="ws://localhost:3000/mcp" />
          </div>
          <div className="flex justify-end gap-2">
            <button onClick={() => setMcpModal(false)} className="px-3 py-1.5 rounded-lg text-sm bg-zinc-700 text-zinc-300 hover:bg-zinc-600">Cancel</button>
            <button onClick={() => setMcpModal(false)} className="px-3 py-1.5 rounded-lg text-sm bg-blue-600 text-white hover:bg-blue-500">Connect</button>
          </div>
        </div>
      </Modal>
    </div>
  );
}
