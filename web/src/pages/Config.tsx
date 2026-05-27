import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { Badge, statusVariant } from '../components/shared/Badge';
import { Modal } from '../components/shared/Modal';
import {
  fetchConfig,
  updateProvider,
  createProvider,
  rotateJwtSecret,
  updateCloudflare,
  connectSaas,
  disconnectSaas,
  login,
  type Provider,
} from '../lib/api';

type Tab = 'providers' | 'channels' | 'cloudflare' | 'saas' | 'system';

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
    mutationFn: (data: typeof form) =>
      initial ? updateProvider(initial.id, data) : createProvider(data),
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

  const { data, isLoading } = useQuery({
    queryKey: ['config'],
    queryFn: fetchConfig,
  });

  const rotateMut = useMutation({
    mutationFn: rotateJwtSecret,
    onSuccess: () => qc.invalidateQueries({ queryKey: ['config'] }),
  });

  const cfMut = useMutation({
    mutationFn: ({ service, enabled }: { service: string; enabled: boolean }) =>
      updateCloudflare(service, enabled),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['config'] }),
  });

  const saasMut = useMutation({
    mutationFn: ({ id, connected }: { id: string; connected: boolean }) =>
      connected ? disconnectSaas(id) : connectSaas(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['config'] }),
  });

  const tabs: { id: Tab; label: string }[] = [
    { id: 'providers', label: 'Providers' },
    { id: 'channels', label: 'Channels' },
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

          {!isLoading && tab === 'channels' && (
            <div className="space-y-2">
              {(data?.channels ?? []).length === 0 ? (
                <div className="py-8 text-center text-zinc-500 text-sm">No channels configured.</div>
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
                    <Toggle checked={ch.enabled} onChange={() => {}} />
                  </div>
                ))
              )}
            </div>
          )}

          {!isLoading && tab === 'cloudflare' && (
            <div className="space-y-2">
              <p className="text-zinc-500 text-sm mb-3">
                Enable or disable Cloudflare services per component.
              </p>
              {Object.entries(data?.cloudflare ?? {}).length === 0 ? (
                <div className="py-8 text-center text-zinc-500 text-sm">No Cloudflare config.</div>
              ) : (
                Object.entries(data?.cloudflare ?? {}).map(([service, enabled]) => (
                  <div key={service} className="flex items-center justify-between p-3 rounded-lg bg-zinc-800">
                    <div>
                      <div className="text-zinc-200 text-sm">{service}</div>
                    </div>
                    <Toggle
                      checked={enabled}
                      onChange={(v) => cfMut.mutate({ service, enabled: v })}
                      disabled={cfMut.isPending}
                    />
                  </div>
                ))
              )}
            </div>
          )}

          {!isLoading && tab === 'saas' && (
            <div className="space-y-2">
              {(data?.saas_connectors ?? []).length === 0 ? (
                <div className="py-8 text-center text-zinc-500 text-sm">No SaaS connectors available.</div>
              ) : (
                (data?.saas_connectors ?? []).map((s) => (
                  <div key={s.id} className="flex items-center gap-3 p-3 rounded-lg bg-zinc-800">
                    {s.icon && <span className="text-xl">{s.icon}</span>}
                    <div className="flex-1">
                      <div className="text-zinc-100 text-sm">{s.name}</div>
                    </div>
                    <Badge variant={s.connected ? 'success' : 'default'}>
                      {s.connected ? 'connected' : 'disconnected'}
                    </Badge>
                    <button
                      onClick={() => saasMut.mutate({ id: s.id, connected: s.connected })}
                      disabled={saasMut.isPending}
                      className={`px-2.5 py-1 text-xs rounded transition-colors disabled:opacity-50 ${
                        s.connected
                          ? 'bg-red-600/20 text-red-400 hover:bg-red-600/30'
                          : 'bg-blue-600/20 text-blue-400 hover:bg-blue-600/30'
                      }`}
                    >
                      {s.connected ? 'Disconnect' : 'Connect'}
                    </button>
                  </div>
                ))
              )}
            </div>
          )}

          {!isLoading && tab === 'system' && (
            <div className="space-y-4">
              <div className="p-4 rounded-lg bg-zinc-800 border border-zinc-700 space-y-3">
                <h3 className="text-zinc-200 text-sm font-medium">Sign in</h3>
                <p className="text-zinc-500 text-xs">
                  Stores a JWT in <code className="text-zinc-400">localStorage.clawz_token</code> for API requests.
                </p>
                <LoginForm />
              </div>
              <div className="grid grid-cols-2 gap-4">
                <div>
                  <label className="block text-zinc-400 text-xs mb-1">Gateway Port</label>
                  <input
                    readOnly
                    className="w-full bg-zinc-800 border border-zinc-700 text-zinc-300 text-sm rounded-lg px-3 py-2 font-mono"
                    value={data?.system.port ?? 8000}
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
                  <label className="block text-zinc-400 text-xs mb-1">JWT Secret</label>
                  <div className="flex gap-2">
                    <input
                      readOnly
                      type="password"
                      className="flex-1 bg-zinc-800 border border-zinc-700 text-zinc-300 text-sm rounded-lg px-3 py-2 font-mono"
                      value={data?.system.jwt_secret_masked ?? '••••••••'}
                    />
                    <button
                      onClick={() => rotateMut.mutate()}
                      disabled={rotateMut.isPending}
                      className="px-3 py-1.5 text-sm bg-yellow-600/20 text-yellow-400 rounded-lg hover:bg-yellow-600/30 disabled:opacity-50 whitespace-nowrap"
                    >
                      {rotateMut.isPending ? '...' : 'Rotate'}
                    </button>
                  </div>
                </div>
              </div>

              <div className="pt-3 border-t border-zinc-800">
                <button
                  onClick={async () => {
                    const { exportConfig: exp } = await import('../lib/api');
                    const cfg = await exp();
                    const blob = new Blob([JSON.stringify(cfg, null, 2)], { type: 'application/json' });
                    const url = URL.createObjectURL(blob);
                    const a = document.createElement('a');
                    a.href = url;
                    a.download = 'clawz-config.json';
                    a.click();
                  }}
                  className="px-3 py-1.5 text-sm bg-zinc-700 text-zinc-300 rounded-lg hover:bg-zinc-600"
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
