import { useEffect, useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { Badge, statusVariant } from '../components/shared/Badge';
import { Modal } from '../components/shared/Modal';
import { DesktopShellSettings } from '../components/DesktopShellSettings';
import {
  fetchConfig,
  fetchCloudProviders,
  fetchCloudDeployments,
  updateProvider,
  createProvider,
  updateChannel,
  createChannel,
  updateSystemConfig,
  exportConfig,
  login,
  fetchWorkspaceSkills,
  createWorkspaceSkill,
  type Provider,
  type WorkspaceSkill,
} from '../lib/api';

type Tab = 'providers' | 'channels' | 'skills' | 'cloud' | 'cloudflare' | 'saas' | 'system';

function ProviderModal({
  open,
  onClose,
  initial,
}: {
  open: boolean;
  onClose: () => void;
  initial?: Provider;
}) {
  const qc = useQueryClient();
  const [form, setForm] = useState({
    name: initial?.name ?? '',
    type: initial?.type ?? 'anthropic',
    api_key: '',
    enabled: initial?.enabled ?? true,
  });
  const [error, setError] = useState('');

  const mutation = useMutation({
    mutationFn: (data: typeof form) => {
      const payload = {
        name: data.name,
        type: data.type,
        enabled: data.enabled,
        api_key: data.api_key.trim() || undefined,
      };
      return initial ? updateProvider(initial.id, payload) : createProvider(payload);
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['config'] });
      onClose();
    },
    onError: (e) => setError(String(e)),
  });

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={initial ? 'Edit Provider' : 'Add Provider'}
      size="md"
      footer={
        <>
          <button onClick={onClose} className="px-3 py-1.5 rounded-lg text-sm bg-zinc-700 text-zinc-300 hover:bg-zinc-600">Cancel</button>
          <button
            onClick={() => { if (!form.name.trim()) { setError('Name required'); return; } mutation.mutate(form); }}
            disabled={mutation.isPending}
            className="px-3 py-1.5 rounded-lg text-sm bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50"
          >
            {mutation.isPending ? 'Saving...' : 'Save'}
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
          <label className="block text-zinc-400 text-xs mb-1">Type</label>
          <select className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500"
            value={form.type} onChange={(e) => setForm((f) => ({ ...f, type: e.target.value }))}>
            {['anthropic', 'openai', 'google', 'mistral', 'local'].map((t) => (
              <option key={t} value={t}>{t}</option>
            ))}
          </select>
        </div>
        <div>
          <label className="block text-zinc-400 text-xs mb-1">
            API Key {initial && <span className="text-zinc-500">(leave blank to keep existing)</span>}
          </label>
          <input
            type="password"
            className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 focus:outline-none focus:border-blue-500 font-mono"
            placeholder={initial?.api_key_masked ?? 'sk-...'}
            value={form.api_key}
            onChange={(e) => setForm((f) => ({ ...f, api_key: e.target.value }))}
          />
        </div>
        <label className="flex items-center gap-2 cursor-pointer">
          <input type="checkbox" checked={form.enabled} onChange={(e) => setForm((f) => ({ ...f, enabled: e.target.checked }))} />
          <span className="text-zinc-300 text-sm">Enabled</span>
        </label>
      </div>
    </Modal>
  );
}

function LoginForm() {
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  const mutation = useMutation({
    mutationFn: () => login(email, password),
    onSuccess: (data) => {
      setMessage(`Signed in as ${data.email}`);
      setError('');
    },
    onError: (e) => {
      setError(String(e));
      setMessage('');
    },
  });

  return (
    <div className="space-y-2">
      <input
        type="email"
        placeholder="email@example.com"
        className="w-full bg-zinc-900 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
        value={email}
        onChange={(e) => setEmail(e.target.value)}
      />
      <input
        type="password"
        placeholder="password"
        className="w-full bg-zinc-900 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
        value={password}
        onChange={(e) => setPassword(e.target.value)}
      />
      <button
        type="button"
        onClick={() => mutation.mutate()}
        disabled={mutation.isPending || !email || !password}
        className="px-3 py-1.5 text-sm bg-blue-600 text-white rounded-lg hover:bg-blue-500 disabled:opacity-50"
      >
        {mutation.isPending ? 'Signing in...' : 'Sign in'}
      </button>
      {message && <p className="text-green-400 text-xs">{message}</p>}
      {error && <p className="text-red-400 text-xs">{error}</p>}
    </div>
  );
}

function Toggle({
  checked,
  onChange,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <button
      onClick={() => onChange(!checked)}
      disabled={disabled}
      className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors disabled:opacity-50 ${
        checked ? 'bg-blue-600' : 'bg-zinc-700'
      }`}
    >
      <span
        className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition-transform ${
          checked ? 'translate-x-4' : 'translate-x-0.5'
        }`}
      />
    </button>
  );
}

export function Config() {
  const qc = useQueryClient();
  const [tab, setTab] = useState<Tab>('providers');
  const [providerModal, setProviderModal] = useState<{ open: boolean; provider?: Provider }>({ open: false });
  const [systemForm, setSystemForm] = useState({
    log_level: 'info',
    max_agents: 100,
    enable_audit: true,
  });
  const { data, isLoading, isError, error } = useQuery({
    queryKey: ['config'],
    queryFn: fetchConfig,
  });

  const { data: cloudProviders = [], isLoading: cloudProvidersLoading } = useQuery({
    queryKey: ['cloud-providers'],
    queryFn: fetchCloudProviders,
    enabled: tab === 'cloud',
  });

  const { data: cloudDeployments = [], isLoading: cloudDeploymentsLoading } = useQuery({
    queryKey: ['cloud-deployments'],
    queryFn: fetchCloudDeployments,
    enabled: tab === 'cloud',
  });

  const { data: skillsData, isLoading: skillsLoading, refetch: refetchSkills } = useQuery({
    queryKey: ['workspace-skills'],
    queryFn: fetchWorkspaceSkills,
    enabled: tab === 'skills',
  });

  const [skillForm, setSkillForm] = useState({ name: '', content: '', description: '' });
  const skillCreateMut = useMutation({
    mutationFn: () =>
      createWorkspaceSkill({
        name: skillForm.name.trim(),
        content: skillForm.content,
        description: skillForm.description.trim() || undefined,
      }),
    onSuccess: () => {
      setSkillForm({ name: '', content: '', description: '' });
      refetchSkills();
    },
  });

  useEffect(() => {
    if (data?.system) {
      setSystemForm({
        log_level: data.system.log_level ?? 'info',
        max_agents: data.system.max_agents ?? 100,
        enable_audit: data.system.enable_audit ?? true,
      });
    }
  }, [data]);

  const channelMut = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      updateChannel(id, { enabled }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['config'] }),
  });

  const channelCreateMut = useMutation({
    mutationFn: (body: { name: string; channel_type: string }) => createChannel(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['config'] }),
  });

  const systemMut = useMutation({
    mutationFn: () => updateSystemConfig(systemForm),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['config'] }),
  });

  const exportMut = useMutation({
    mutationFn: exportConfig,
    onSuccess: (cfg) => {
      const blob = new Blob([JSON.stringify(cfg, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = 'clawz-config.json';
      a.click();
      URL.revokeObjectURL(url);
    },
  });

  const tabs: { id: Tab; label: string }[] = [
    { id: 'providers', label: 'Providers' },
    { id: 'channels', label: 'Channels' },
    { id: 'skills', label: 'Skills' },
    { id: 'cloud', label: 'Cloud' },
    { id: 'cloudflare', label: 'Cloudflare' },
    { id: 'saas', label: 'SaaS' },
    { id: 'system', label: 'System' },
  ];

  return (
    <div className="flex flex-col h-full overflow-y-auto p-4 gap-4">
      <h2 className="text-zinc-100 font-semibold">Config</h2>

      <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800 overflow-hidden flex-1">
        <div className="flex border-b border-zinc-800">
          {tabs.map((t) => (
            <button
              key={t.id}
              onClick={() => setTab(t.id)}
              className={`px-4 py-2.5 text-sm font-medium transition-colors ${
                tab === t.id ? 'text-blue-400 border-b-2 border-blue-500' : 'text-zinc-500 hover:text-zinc-300'
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>

        <div className="p-4">
          {isError && (
            <div className="mb-4 px-3 py-2 bg-red-900/30 border border-red-700 rounded text-red-400 text-sm">
              Failed to load config: {String(error)}
            </div>
          )}
          {isLoading && (
            <div className="space-y-2">
              {Array.from({ length: 4 }).map((_, i) => (
                <div key={i} className="h-12 bg-zinc-800 rounded-lg animate-pulse" />
              ))}
            </div>
          )}

          {!isLoading && tab === 'providers' && (
            <div className="space-y-3">
              <div className="flex justify-end">
                <button
                  onClick={() => setProviderModal({ open: true })}
                  className="px-3 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500"
                >
                  + Add Provider
                </button>
              </div>
              {(data?.providers ?? []).length === 0 ? (
                <div className="py-8 text-center text-zinc-500 text-sm">No providers configured.</div>
              ) : (
                <div className="space-y-2">
                  {(data?.providers ?? []).map((p) => (
                    <div key={p.id} className="flex items-center gap-3 p-3 rounded-lg bg-zinc-800">
                      <div className="w-8 h-8 rounded-lg bg-zinc-700 flex items-center justify-center text-xs text-zinc-300 font-bold uppercase">
                        {p.type.slice(0, 2)}
                      </div>
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2">
                          <span className="text-zinc-100 text-sm font-medium">{p.name}</span>
                          <Badge variant="default">{p.type}</Badge>
                          <Badge variant={p.enabled ? 'success' : 'default'}>
                            {p.enabled ? 'enabled' : 'disabled'}
                          </Badge>
                        </div>
                        {p.api_key_masked && (
                          <div className="text-zinc-500 text-xs font-mono">{p.api_key_masked}</div>
                        )}
                      </div>
                      <button
                        onClick={() => setProviderModal({ open: true, provider: p })}
                        className="px-2 py-1 text-xs rounded bg-zinc-700 text-zinc-300 hover:bg-zinc-600"
                      >
                        Edit
                      </button>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}

          {!isLoading && tab === 'skills' && (
            <div className="space-y-4">
              {skillsData?.root && (
                <div className="text-xs text-zinc-500 font-mono truncate">
                  Workspace: {skillsData.root}
                </div>
              )}
              <div className="grid gap-3 md:grid-cols-2">
                <div className="space-y-2">
                  <div className="text-xs text-zinc-500 uppercase tracking-wider">
                    Installed skills ({skillsData?.skills.length ?? 0})
                  </div>
                  {skillsLoading ? (
                    <div className="text-zinc-500 text-sm">Loading…</div>
                  ) : (skillsData?.skills ?? []).length === 0 ? (
                    <div className="text-zinc-500 text-sm py-4">No skills in workspace yet.</div>
                  ) : (
                    <div className="space-y-2 max-h-64 overflow-y-auto">
                      {(skillsData?.skills ?? []).map((s: WorkspaceSkill) => (
                        <div key={s.name} className="p-3 rounded-lg bg-zinc-800">
                          <div className="text-zinc-100 text-sm font-medium">{s.name}</div>
                          {s.description && (
                            <div className="text-zinc-500 text-xs mt-1">{s.description}</div>
                          )}
                          {s.path && (
                            <div className="text-zinc-600 text-[10px] font-mono mt-1 truncate">
                              {s.path}
                            </div>
                          )}
                        </div>
                      ))}
                    </div>
                  )}
                </div>
                <div className="space-y-2 p-3 rounded-lg border border-zinc-800">
                  <div className="text-xs text-zinc-500 uppercase tracking-wider">Add skill</div>
                  <input
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
                    placeholder="skill-name"
                    value={skillForm.name}
                    onChange={(e) => setSkillForm((f) => ({ ...f, name: e.target.value }))}
                  />
                  <input
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
                    placeholder="Short description (optional)"
                    value={skillForm.description}
                    onChange={(e) =>
                      setSkillForm((f) => ({ ...f, description: e.target.value }))
                    }
                  />
                  <textarea
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 min-h-[120px] font-mono"
                    placeholder="# SKILL.md content"
                    value={skillForm.content}
                    onChange={(e) => setSkillForm((f) => ({ ...f, content: e.target.value }))}
                  />
                  <button
                    type="button"
                    disabled={
                      skillCreateMut.isPending ||
                      !skillForm.name.trim() ||
                      !skillForm.content.trim()
                    }
                    onClick={() => skillCreateMut.mutate()}
                    className="px-3 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500 disabled:opacity-50"
                  >
                    {skillCreateMut.isPending ? 'Saving…' : 'Create skill'}
                  </button>
                </div>
              </div>
            </div>
          )}

          {!isLoading && tab === 'channels' && (
            <div className="space-y-3">
              <div className="flex justify-end">
                <button
                  onClick={() => {
                    const name = window.prompt('Channel name');
                    const channel_type = window.prompt('Type (webhook, slack, email, telegram)', 'webhook');
                    if (name?.trim() && channel_type?.trim()) {
                      channelCreateMut.mutate({
                        name: name.trim(),
                        channel_type: channel_type.trim(),
                      });
                    }
                  }}
                  disabled={channelCreateMut.isPending}
                  className="px-3 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500 disabled:opacity-50"
                >
                  + Add Channel
                </button>
              </div>
              {(data?.channels ?? []).length === 0 ? (
                <div className="py-8 text-center text-zinc-500 text-sm">
                  No channels yet. Add a webhook, Slack, or other integration.
                </div>
              ) : (
                (data?.channels ?? []).map((ch) => (
                  <div key={ch.id} className="flex items-center gap-3 p-3 rounded-lg bg-zinc-800">
                    <div className="flex-1">
                      <div className="flex items-center gap-2">
                        <span className="text-zinc-100 text-sm">{ch.name}</span>
                        <Badge variant="default">{ch.type}</Badge>
                        <Badge variant={statusVariant(ch.status)}>{ch.status}</Badge>
                      </div>
                    </div>
                    <Toggle
                      checked={ch.enabled}
                      onChange={(v) => channelMut.mutate({ id: ch.id, enabled: v })}
                      disabled={channelMut.isPending}
                    />
                  </div>
                ))
              )}
            </div>
          )}

          {!isLoading && tab === 'cloud' && (
            <div className="space-y-6">
              <div>
                <h3 className="text-zinc-300 text-sm font-medium mb-3">Cloud providers</h3>
                {cloudProvidersLoading ? (
                  <div className="space-y-2">
                    {Array.from({ length: 3 }).map((_, i) => (
                      <div key={i} className="h-12 bg-zinc-800 rounded-lg animate-pulse" />
                    ))}
                  </div>
                ) : cloudProviders.length === 0 ? (
                  <div className="py-6 text-center text-zinc-500 text-sm">No cloud adapters registered.</div>
                ) : (
                  <div className="space-y-2">
                    {cloudProviders.map((p) => (
                      <div key={p.id} className="flex items-center gap-3 p-3 rounded-lg bg-zinc-800">
                        <div className="flex-1 min-w-0">
                          <div className="text-zinc-100 text-sm font-medium">{p.display_name}</div>
                          <div className="text-zinc-500 text-xs font-mono">{p.id}</div>
                        </div>
                        <div className="flex flex-wrap gap-1 justify-end">
                          {p.deploy_modes.map((mode) => (
                            <Badge key={mode} variant="default">{mode}</Badge>
                          ))}
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
              <div>
                <h3 className="text-zinc-300 text-sm font-medium mb-3">Cloud deployments</h3>
                {cloudDeploymentsLoading ? (
                  <div className="h-24 bg-zinc-800 rounded-lg animate-pulse" />
                ) : cloudDeployments.length === 0 ? (
                  <div className="py-6 text-center text-zinc-500 text-sm">
                    No cloud deployments yet. Use POST /api/v1/cloud/deploy from the API or CLI.
                  </div>
                ) : (
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-zinc-800">
                        <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">ID</th>
                        <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Provider</th>
                        <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Status</th>
                        <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">URL</th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-zinc-800">
                      {cloudDeployments.map((d) => (
                        <tr key={d.id} className="hover:bg-zinc-800/50">
                          <td className="px-3 py-2.5 text-zinc-300 font-mono text-xs">{d.id.slice(0, 8)}…</td>
                          <td className="px-3 py-2.5 text-zinc-400 text-xs">{d.provider_id}</td>
                          <td className="px-3 py-2.5">
                            <Badge variant={statusVariant(String(d.status))}>{String(d.status)}</Badge>
                          </td>
                          <td className="px-3 py-2.5">
                            {d.url ? (
                              <a
                                href={d.url}
                                target="_blank"
                                rel="noopener noreferrer"
                                className="text-blue-400 text-xs hover:underline truncate block max-w-xs"
                              >
                                {d.url}
                              </a>
                            ) : (
                              <span className="text-zinc-500 text-xs">—</span>
                            )}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            </div>
          )}

          {!isLoading && tab === 'cloudflare' && (
            <div className="space-y-2">
              <p className="text-zinc-500 text-sm mb-3">
                Status is read from gateway environment variables (
                <code className="text-zinc-400">CLOUDFLARE_API_TOKEN</code>,{' '}
                <code className="text-zinc-400">CLAWZ_CF_*_ENABLED</code>).
              </p>
              {Object.entries(data?.cloudflare ?? {}).filter(([k]) => k !== 'configured').length === 0 ? (
                <div className="py-8 text-center text-zinc-500 text-sm">No Cloudflare config.</div>
              ) : (
                Object.entries(data?.cloudflare ?? {})
                  .filter(([k]) => k !== 'configured')
                  .map(([service, enabled]) => (
                  <div key={service} className="flex items-center justify-between p-3 rounded-lg bg-zinc-800">
                    <div>
                      <div className="text-zinc-200 text-sm capitalize">{service.replace(/_/g, ' ')}</div>
                    </div>
                    <Badge variant={enabled ? 'success' : 'default'}>
                      {enabled ? 'enabled' : 'off'}
                    </Badge>
                  </div>
                ))
              )}
            </div>
          )}

          {!isLoading && tab === 'saas' && (
            <div className="space-y-2">
              <p className="text-zinc-500 text-sm mb-3">
                Connectors are enabled on the gateway via environment variables (OAuth UI coming later).
              </p>
              {(data?.saas_connectors ?? []).length === 0 ? (
                <div className="py-8 text-center text-zinc-500 text-sm">No SaaS connectors available.</div>
              ) : (
                (data?.saas_connectors ?? []).map((s) => (
                  <div key={s.id} className="flex items-center gap-3 p-3 rounded-lg bg-zinc-800">
                    {s.icon && <span className="text-xl">{s.icon}</span>}
                    <div className="flex-1 min-w-0">
                      <div className="text-zinc-100 text-sm">{s.name}</div>
                      {s.env_hint && (
                        <div className="text-zinc-500 text-xs font-mono truncate">{s.env_hint}</div>
                      )}
                    </div>
                    <Badge variant={s.connected ? 'success' : 'default'}>
                      {s.connected ? 'connected' : 'disconnected'}
                    </Badge>
                  </div>
                ))
              )}
            </div>
          )}

          {!isLoading && tab === 'system' && (
            <div className="space-y-4">
              <DesktopShellSettings />
              {!data?.system.auth_disabled && (
                <div className="p-4 rounded-lg bg-zinc-800 border border-zinc-700 space-y-3">
                  <h3 className="text-zinc-200 text-sm font-medium">Sign in</h3>
                  <p className="text-zinc-500 text-xs">
                    Stores a JWT in <code className="text-zinc-400">localStorage.clawz_token</code> for API requests.
                  </p>
                  <LoginForm />
                </div>
              )}
              <div className="grid grid-cols-2 gap-4">
                <div>
                  <label className="block text-zinc-400 text-xs mb-1">Gateway Port</label>
                  <input
                    readOnly
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-300 text-sm rounded-lg px-3 py-2 font-mono"
                    value={data?.system.port ?? 3000}
                  />
                </div>
                <div>
                  <label className="block text-zinc-400 text-xs mb-1">Gateway Version</label>
                  <input
                    readOnly
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-300 text-sm rounded-lg px-3 py-2 font-mono"
                    value={data?.system.gateway_version ?? '—'}
                  />
                </div>
                <div>
                  <label className="block text-zinc-400 text-xs mb-1">Default Provider</label>
                  <input
                    readOnly
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-300 text-sm rounded-lg px-3 py-2 font-mono"
                    value={data?.default_provider ?? '—'}
                  />
                </div>
                <div>
                  <label className="block text-zinc-400 text-xs mb-1">Docker Registry</label>
                  <input
                    readOnly
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-300 text-sm rounded-lg px-3 py-2 font-mono"
                    value={data?.docker_registry ?? '—'}
                  />
                </div>
                <div>
                  <label className="block text-zinc-400 text-xs mb-1">Log Level</label>
                  <select
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
                    value={systemForm.log_level}
                    onChange={(e) => setSystemForm((f) => ({ ...f, log_level: e.target.value }))}
                  >
                    {['trace', 'debug', 'info', 'warn', 'error'].map((l) => (
                      <option key={l} value={l}>{l}</option>
                    ))}
                  </select>
                </div>
                <div>
                  <label className="block text-zinc-400 text-xs mb-1">Max Agents</label>
                  <input
                    type="number"
                    min={1}
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
                    value={systemForm.max_agents}
                    onChange={(e) =>
                      setSystemForm((f) => ({ ...f, max_agents: Number(e.target.value) || 1 }))
                    }
                  />
                </div>
                <div className="col-span-2">
                  <label className="flex items-center gap-2 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={systemForm.enable_audit}
                      onChange={(e) =>
                        setSystemForm((f) => ({ ...f, enable_audit: e.target.checked }))
                      }
                    />
                    <span className="text-zinc-300 text-sm">Enable audit log</span>
                  </label>
                </div>
                <div className="col-span-2">
                  <label className="block text-zinc-400 text-xs mb-1">JWT Secret</label>
                  <input
                    readOnly
                    type="password"
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-300 text-sm rounded-lg px-3 py-2 font-mono"
                    value={data?.system.jwt_secret_masked ?? '••••••••'}
                  />
                  <p className="text-zinc-500 text-xs mt-1">
                    Set <code className="text-zinc-400">CLAWZ_JWT_SECRET</code> on the gateway host.
                  </p>
                </div>
              </div>

              <p className="text-zinc-500 text-xs">
                System settings are held in gateway memory until restart. Set{' '}
                <code className="text-zinc-400">LOG_LEVEL</code>,{' '}
                <code className="text-zinc-400">CLAWZ_MAX_AGENTS</code> in the environment for persistence.
              </p>
              <div className="flex gap-2 pt-3 border-t border-zinc-800">
                <button
                  onClick={() => systemMut.mutate()}
                  disabled={systemMut.isPending}
                  className="px-3 py-1.5 text-sm bg-blue-600 text-white rounded-lg hover:bg-blue-500 disabled:opacity-50"
                >
                  {systemMut.isPending ? 'Saving...' : 'Save System Settings'}
                </button>
                <button
                  onClick={() => exportMut.mutate()}
                  disabled={exportMut.isPending}
                  className="px-3 py-1.5 text-sm bg-zinc-700 text-zinc-300 rounded-lg hover:bg-zinc-600 disabled:opacity-50"
                >
                  Export Config
                </button>
              </div>
            </div>
          )}
        </div>
      </div>

      <ProviderModal
        open={providerModal.open}
        onClose={() => setProviderModal({ open: false })}
        initial={providerModal.provider}
      />
    </div>
  );
}
