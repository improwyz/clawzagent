import { useEffect, useState } from 'react';
import {
  getShellConfig,
  isTauriShell,
  setShellConfig,
  type ShellMode,
} from '../lib/shell';
import { initShellApi, setStoredApiKey } from '../lib/api';

export function DesktopShellSettings() {
  const [visible, setVisible] = useState(false);
  const [mode, setMode] = useState<ShellMode>('gateway');
  const [gatewayUrl, setGatewayUrl] = useState('http://127.0.0.1:3000');
  const [apiKey, setApiKey] = useState('');
  const [apiKeySet, setApiKeySet] = useState(false);
  const [status, setStatus] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    if (!isTauriShell()) return;
    setVisible(true);
    getShellConfig()
      .then((cfg) => {
        setMode(cfg.mode === 'standalone' ? 'standalone' : 'gateway');
        setGatewayUrl(cfg.gateway_url ?? 'http://127.0.0.1:3000');
        setApiKeySet(cfg.api_key_set);
        setStatus(cfg.connection_status);
      })
      .catch((e) => setError(String(e)));
  }, []);

  if (!visible) return null;

  async function onSave() {
    setSaving(true);
    setError('');
    try {
      const cfg = await setShellConfig({
        mode,
        gateway_url: gatewayUrl.trim() || undefined,
        api_key: apiKey.trim(),
      });
      if (apiKey.trim()) {
        setStoredApiKey(apiKey.trim());
      }
      setApiKeySet(cfg.api_key_set);
      setApiKey('');
      await initShellApi();
      setStatus('saved');
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="mb-6 p-4 rounded-lg border border-zinc-700 bg-zinc-900/50">
      <h3 className="text-zinc-100 font-medium mb-1">Desktop shell</h3>
      <p className="text-zinc-500 text-xs mb-4">
        Connection mode for the packaged app. Gateway mode uses REST against your ClawZ gateway;
        standalone keeps the embedded worker for IPC (full dashboard still needs a reachable gateway URL).
      </p>
      {error && (
        <div className="mb-3 px-3 py-2 bg-red-900/30 border border-red-700 rounded text-red-400 text-sm">
          {error}
        </div>
      )}
      <div className="grid grid-cols-2 gap-4">
        <div>
          <label className="block text-zinc-400 text-xs mb-1">Shell mode</label>
          <select
            className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
            value={mode}
            onChange={(e) => setMode(e.target.value as ShellMode)}
          >
            <option value="gateway">Gateway (REST)</option>
            <option value="standalone">Standalone (embedded worker)</option>
          </select>
        </div>
        <div>
          <label className="block text-zinc-400 text-xs mb-1">Gateway URL</label>
          <input
            className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 font-mono"
            value={gatewayUrl}
            onChange={(e) => setGatewayUrl(e.target.value)}
            placeholder="http://127.0.0.1:3000"
          />
        </div>
        <div className="col-span-2">
          <label className="block text-zinc-400 text-xs mb-1">
            API key {apiKeySet ? '(stored in OS keyring)' : ''}
          </label>
          <input
            type="password"
            className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 font-mono"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={apiKeySet ? 'Leave blank to keep current key' : 'Bearer / X-API-Key value'}
          />
        </div>
      </div>
      <div className="flex items-center gap-3 mt-4">
        <button
          type="button"
          onClick={onSave}
          disabled={saving}
          className="px-3 py-1.5 text-sm bg-blue-600 text-white rounded-lg hover:bg-blue-500 disabled:opacity-50"
        >
          {saving ? 'Saving…' : 'Save shell settings'}
        </button>
        {status && <span className="text-zinc-500 text-xs">Status: {status}</span>}
      </div>
    </div>
  );
}
