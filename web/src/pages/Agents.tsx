import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { StatCard } from '../components/widgets/StatCard';
import { Badge, statusVariant } from '../components/shared/Badge';
import { Modal, ConfirmModal } from '../components/shared/Modal';
import { EmptyState } from '../components/shared/EmptyState';
import { ChatPanel } from '../components/chat/ChatPanel';
import {
  fetchAgents,
  createAgent,
  deleteAgent,
  createRoom,
  fetchRooms,
  type Agent,
  type Room,
} from '../lib/api';

const MODELS = [
  'claude-opus-4-5',
  'claude-sonnet-4-5',
  'claude-haiku-3-5',
  'gpt-4o',
  'gpt-4o-mini',
  'gemini-1.5-pro',
];

const AVAILABLE_TOOLS = [
  'web_search', 'code_exec', 'file_ops', 'database', 'api_call', 'email', 'calendar',
];

interface AgentFormData {
  name: string;
  model: string;
  system_prompt: string;
  tools: string[];
}

function CreateAgentModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient();
  const [form, setForm] = useState<AgentFormData>({
    name: '',
    model: MODELS[1],
    system_prompt: '',
    tools: [],
  });
  const [error, setError] = useState('');

  const mutation = useMutation({
    mutationFn: createAgent,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['agents'] });
      onClose();
      setForm({ name: '', model: MODELS[1], system_prompt: '', tools: [] });
    },
    onError: (e) => setError(String(e)),
  });

  const toggleTool = (tool: string) => {
    setForm((f) => ({
      ...f,
      tools: f.tools.includes(tool) ? f.tools.filter((t) => t !== tool) : [...f.tools, tool],
    }));
  };

  const submit = () => {
    if (!form.name.trim()) { setError('Name is required'); return; }
    setError('');
    mutation.mutate(form);
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Create Agent"
      size="lg"
      footer={
        <>
          <button
            onClick={onClose}
            className="px-3 py-1.5 rounded-lg text-sm bg-zinc-700 text-zinc-300 hover:bg-zinc-600 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={submit}
            disabled={mutation.isPending}
            className="px-3 py-1.5 rounded-lg text-sm bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 transition-colors"
          >
            {mutation.isPending ? 'Creating...' : 'Create Agent'}
          </button>
        </>
      }
    >
      <div className="space-y-4">
        {error && (
          <div className="px-3 py-2 bg-red-900/30 border border-red-700 rounded text-red-400 text-sm">
            {error}
          </div>
        )}
        <div>
          <label className="block text-zinc-400 text-xs font-medium mb-1.5">Name *</label>
          <input
            className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500"
            placeholder="my-agent"
            value={form.name}
            onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
          />
        </div>
        <div>
          <label className="block text-zinc-400 text-xs font-medium mb-1.5">Model</label>
          <select
            className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500"
            value={form.model}
            onChange={(e) => setForm((f) => ({ ...f, model: e.target.value }))}
          >
            {MODELS.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        </div>
        <div>
          <label className="block text-zinc-400 text-xs font-medium mb-1.5">System Prompt</label>
          <textarea
            className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500 resize-none h-24 font-mono"
            placeholder="You are a helpful assistant..."
            value={form.system_prompt}
            onChange={(e) => setForm((f) => ({ ...f, system_prompt: e.target.value }))}
          />
        </div>
        <div>
          <label className="block text-zinc-400 text-xs font-medium mb-1.5">Tools</label>
          <div className="flex flex-wrap gap-2">
            {AVAILABLE_TOOLS.map((tool) => (
              <button
                key={tool}
                onClick={() => toggleTool(tool)}
                className={`px-2.5 py-1 rounded-lg text-xs border transition-colors ${
                  form.tools.includes(tool)
                    ? 'bg-blue-600/20 border-blue-500 text-blue-400'
                    : 'bg-zinc-800 border-zinc-700 text-zinc-400 hover:border-zinc-500'
                }`}
              >
                {tool}
              </button>
            ))}
          </div>
        </div>
      </div>
    </Modal>
  );
}

function AgentRow({ agent, onDelete, onChat, onTeamRoom, selected }: {
  agent: Agent;
  onDelete: (id: string) => void;
  onChat: (agent: Agent) => void;
  onTeamRoom: (agent: Agent) => void;
  selected: boolean;
}) {
  const [expanded, setExpanded] = useState(false);

  return (
    <>
      <tr
        className="hover:bg-zinc-800/50 transition-colors cursor-pointer"
        onClick={() => setExpanded(!expanded)}
      >
        <td className="px-3 py-2.5">
          <div className="flex items-center gap-2">
            <div className="w-1.5 h-1.5 rounded-full bg-zinc-600 flex-shrink-0" />
            <span className="text-zinc-100 text-sm font-medium">{agent.name}</span>
          </div>
        </td>
        <td className="px-3 py-2.5 text-zinc-400 text-xs font-mono">{agent.model}</td>
        <td className="px-3 py-2.5">
          <Badge variant={statusVariant(agent.status)}>{agent.status}</Badge>
        </td>
        <td className="px-3 py-2.5 text-zinc-500 text-xs">
          {agent.last_active ? new Date(agent.last_active).toLocaleString() : '—'}
        </td>
        <td className="px-3 py-2.5">
          <div className="flex items-center gap-1">
            <button
              onClick={(e) => { e.stopPropagation(); onChat(agent); }}
              className={`px-2 py-1 text-xs rounded transition-colors ${
                selected
                  ? 'bg-blue-600 text-white'
                  : 'bg-blue-600/20 text-blue-400 hover:bg-blue-600/30'
              }`}
            >
              Run
            </button>
            <button
              onClick={(e) => { e.stopPropagation(); onTeamRoom(agent); }}
              className="px-2 py-1 text-xs rounded bg-purple-600/20 text-purple-400 hover:bg-purple-600/30 transition-colors"
              title="Open agent team room"
            >
              Team
            </button>
            <button
              onClick={(e) => { e.stopPropagation(); onDelete(agent.id); }}
              className="px-2 py-1 text-xs rounded bg-red-600/20 text-red-400 hover:bg-red-600/30 transition-colors"
            >
              Delete
            </button>
          </div>
        </td>
      </tr>
      {expanded && (
        <tr className="bg-zinc-900/50">
          <td colSpan={5} className="px-4 py-3">
            <div className="grid grid-cols-3 gap-4 text-sm">
              <div>
                <div className="text-zinc-500 text-xs uppercase tracking-wider mb-1">System Prompt</div>
                <div className="text-zinc-300 text-xs font-mono bg-zinc-800 rounded p-2 max-h-24 overflow-y-auto">
                  {agent.system_prompt || 'No system prompt'}
                </div>
              </div>
              <div>
                <div className="text-zinc-500 text-xs uppercase tracking-wider mb-1">Tools</div>
                <div className="flex flex-wrap gap-1">
                  {(agent.tools ?? []).length === 0 ? (
                    <span className="text-zinc-500 text-xs">None</span>
                  ) : (
                    (agent.tools ?? []).map((t) => (
                      <Badge key={t} variant="info">{t}</Badge>
                    ))
                  )}
                </div>
              </div>
              <div>
                <div className="text-zinc-500 text-xs uppercase tracking-wider mb-1">Stats</div>
                <div className="space-y-1 text-xs">
                  <div className="flex justify-between">
                    <span className="text-zinc-500">Conversations</span>
                    <span className="text-zinc-300 font-mono">{agent.conversation_count ?? 0}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-zinc-500">Cost (total)</span>
                    <span className="text-zinc-300 font-mono">
                      ${(agent.cost_total ?? 0).toFixed(4)}
                    </span>
                  </div>
                </div>
              </div>
            </div>
          </td>
        </tr>
      )}
    </>
  );
}

export function Agents() {
  const qc = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [chatAgent, setChatAgent] = useState<Agent | null>(null);
  const [chatRoom, setChatRoom] = useState<Room | null>(null);
  const [roomLoading, setRoomLoading] = useState(false);

  const { data: agents = [], isLoading, error } = useQuery({
    queryKey: ['agents'],
    queryFn: fetchAgents,
    refetchInterval: 15_000,
  });

  const { data: rooms = [] } = useQuery({
    queryKey: ['rooms'],
    queryFn: fetchRooms,
    refetchInterval: 30_000,
  });

  const deleteMutation = useMutation({
    mutationFn: deleteAgent,
    onSuccess: () => qc.invalidateQueries({ queryKey: ['agents'] }),
  });

  const openTeamRoom = async (leader: Agent) => {
    setRoomLoading(true);
    try {
      const otherAgents = agents.filter((a) => a.id !== leader.id).slice(0, 2);
      const room = await createRoom({
        title: `${leader.name} team`,
        room_type: 'agent_team',
        orchestration_mode: 'team',
        primary_agent_id: leader.id,
        participants: [
          { participant_type: 'agent', participant_id: leader.id, role: 'leader' },
          ...otherAgents.map((a) => ({
            participant_type: 'agent' as const,
            participant_id: a.id,
            role: 'member' as const,
          })),
        ],
      });
      qc.invalidateQueries({ queryKey: ['rooms'] });
      setChatAgent(null);
      setChatRoom(room);
    } catch (err) {
      console.error('Failed to create team room:', err);
    } finally {
      setRoomLoading(false);
    }
  };

  const chatOpen = Boolean(chatAgent || chatRoom);

  const idle = agents.filter((a) => a.status === 'idle').length;
  const running = agents.filter((a) => a.status === 'running').length;
  const errored = agents.filter((a) => a.status === 'error').length;

  return (
    <div className="flex h-full overflow-hidden">
      <div className={`flex flex-col flex-1 min-w-0 overflow-y-auto p-4 gap-4 ${chatOpen ? 'border-r border-zinc-800' : ''}`}>
        {/* Header */}
        <div className="flex items-center justify-between">
          <h2 className="text-zinc-100 font-semibold">Agents</h2>
          <button
            onClick={() => setCreateOpen(true)}
            className="px-3 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500 transition-colors"
          >
            + Create Agent
          </button>
        </div>

        {rooms.length > 0 && (
          <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800 p-3">
            <div className="text-xs text-zinc-500 uppercase tracking-wider mb-2">Rooms</div>
            <div className="flex flex-wrap gap-2">
              {rooms.map((room) => (
                <button
                  key={room.id}
                  onClick={() => {
                    setChatAgent(null);
                    setChatRoom(chatRoom?.id === room.id ? null : room);
                  }}
                  className={`px-2.5 py-1 rounded-lg text-xs border transition-colors ${
                    chatRoom?.id === room.id
                      ? 'bg-purple-600/30 border-purple-500 text-purple-300'
                      : 'bg-zinc-800 border-zinc-700 text-zinc-400 hover:border-zinc-500'
                  }`}
                >
                  {room.title ?? room.id.slice(0, 8)}
                  <span className="ml-1 opacity-60">{room.room_type}</span>
                </button>
              ))}
            </div>
          </div>
        )}

        {/* Stats */}
        <div className="grid grid-cols-4 gap-3">
          <StatCard label="Total" value={agents.length} loading={isLoading} />
          <StatCard label="Running" value={running} loading={isLoading} variant="success" />
          <StatCard label="Idle" value={idle} loading={isLoading} />
          <StatCard label="Error" value={errored} loading={isLoading} variant="error" />
        </div>

        {/* Table */}
        <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800 overflow-hidden">
          {error ? (
            <div className="p-6 text-center text-red-400 text-sm">
              Failed to load agents. Is the gateway running?
            </div>
          ) : (
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-zinc-800">
                  <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2.5">Name</th>
                  <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2.5">Model</th>
                  <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2.5">Status</th>
                  <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2.5">Last Active</th>
                  <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2.5">Actions</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-zinc-800">
                {isLoading
                  ? Array.from({ length: 5 }).map((_, i) => (
                    <tr key={i}>
                      {[1,2,3,4,5].map((j) => (
                        <td key={j} className="px-3 py-2.5">
                          <div className="h-3 bg-zinc-800 rounded animate-pulse w-3/4" />
                        </td>
                      ))}
                    </tr>
                  ))
                  : agents.length === 0
                  ? (
                    <tr>
                      <td colSpan={5}>
                        <EmptyState
                          icon="🤖"
                          title="No agents yet"
                          description="Create your first agent to get started."
                          action={
                            <button
                              onClick={() => setCreateOpen(true)}
                              className="px-3 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500"
                            >
                              Create Agent
                            </button>
                          }
                        />
                      </td>
                    </tr>
                  )
                  : agents.map((agent) => (
                    <AgentRow
                      key={agent.id}
                      agent={agent}
                      onDelete={(id) => setDeleteTarget(id)}
                      onChat={(a) => {
                        setChatRoom(null);
                        setChatAgent(chatAgent?.id === a.id ? null : a);
                      }}
                      onTeamRoom={openTeamRoom}
                      selected={chatAgent?.id === agent.id}
                    />
                  ))}
              </tbody>
            </table>
          )}
        </div>
      </div>

      {/* Chat panel */}
      {chatOpen && (
        <div className="w-96 flex flex-col flex-shrink-0 bg-zinc-900">
          <div className="flex items-center justify-between px-3 py-2.5 border-b border-zinc-800">
            <div className="text-zinc-200 text-sm font-medium truncate">
              {chatRoom
                ? (chatRoom.title ?? `Room ${chatRoom.id.slice(0, 8)}`)
                : chatAgent?.name}
            </div>
            <button
              onClick={() => {
                setChatAgent(null);
                setChatRoom(null);
              }}
              className="text-zinc-500 hover:text-zinc-300 text-xs flex-shrink-0"
            >
              ✕
            </button>
          </div>
          <div className="flex-1 overflow-hidden">
            {roomLoading ? (
              <div className="p-4 text-zinc-500 text-sm">Creating room…</div>
            ) : chatRoom ? (
              <ChatPanel
                roomId={chatRoom.id}
              />
            ) : chatAgent ? (
              <ChatPanel agentId={chatAgent.id} agentName={chatAgent.name} />
            ) : null}
          </div>
        </div>
      )}

      <CreateAgentModal open={createOpen} onClose={() => setCreateOpen(false)} />
      <ConfirmModal
        open={deleteTarget !== null}
        onClose={() => setDeleteTarget(null)}
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget);
        }}
        title="Delete Agent"
        message="Are you sure you want to delete this agent? This cannot be undone."
        confirmLabel="Delete"
        danger
      />
    </div>
  );
}
