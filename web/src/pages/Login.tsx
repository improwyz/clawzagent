import { useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { useNavigate } from 'react-router-dom';
import { ClawzLogo } from '../components/shared/ClawzLogo';
import {
  login,
  register,
  setStoredApiKey,
  getStoredAuthToken,
  getStoredApiKey,
  clearAuthToken,
} from '../lib/api';

export function Login() {
  const navigate = useNavigate();
  const [mode, setMode] = useState<'login' | 'register' | 'apikey'>('login');
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [apiKey, setApiKey] = useState(getStoredApiKey() ?? '');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  const loginMut = useMutation({
    mutationFn: () => login(email, password),
    onSuccess: (data) => {
      setMessage(`Signed in as ${data.email}`);
      setError('');
      navigate('/');
    },
    onError: (e) => {
      setError(String(e));
      setMessage('');
    },
  });

  const registerMut = useMutation({
    mutationFn: () => register(email, password),
    onSuccess: (data) => {
      setMessage(
        data.api_key
          ? `Account created. API key (save now): ${data.api_key}`
          : `Account created as ${data.email}`,
      );
      setError('');
      navigate('/');
    },
    onError: (e) => {
      setError(String(e));
      setMessage('');
    },
  });

  const token = getStoredAuthToken();

  return (
    <div className="min-h-screen bg-zinc-950 flex items-center justify-center p-4">
      <div className="w-full max-w-md widget-3d rounded-xl bg-zinc-900 border border-zinc-800 p-6 space-y-5">
        <div className="flex flex-col items-center gap-2">
          <ClawzLogo variant="full" theme="dark" className="h-8 w-auto" />
          <p className="text-zinc-500 text-sm text-center">
            Sign in to ClawZ when gateway auth is enabled
          </p>
        </div>

        <div className="flex gap-1 p-1 bg-zinc-800 rounded-lg">
          {(['login', 'register', 'apikey'] as const).map((m) => (
            <button
              key={m}
              type="button"
              onClick={() => { setMode(m); setError(''); }}
              className={`flex-1 py-1.5 text-xs rounded-md transition-colors ${
                mode === m ? 'bg-blue-600 text-white' : 'text-zinc-400 hover:text-zinc-200'
              }`}
            >
              {m === 'apikey' ? 'API key' : m === 'login' ? 'Sign in' : 'Register'}
            </button>
          ))}
        </div>

        {(mode === 'login' || mode === 'register') && (
          <div className="space-y-3">
            <input
              type="email"
              placeholder="email@example.com"
              className="w-full bg-zinc-950 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
            <input
              type="password"
              placeholder={mode === 'register' ? 'password (min 8 chars)' : 'password'}
              className="w-full bg-zinc-950 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            <button
              type="button"
              disabled={
                (mode === 'login' ? loginMut : registerMut).isPending ||
                !email ||
                !password ||
                (mode === 'register' && password.length < 8)
              }
              onClick={() => (mode === 'login' ? loginMut : registerMut).mutate()}
              className="w-full py-2 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500 disabled:opacity-50"
            >
              {(mode === 'login' ? loginMut : registerMut).isPending
                ? 'Please wait…'
                : mode === 'login'
                  ? 'Sign in'
                  : 'Create account'}
            </button>
          </div>
        )}

        {mode === 'apikey' && (
          <div className="space-y-3">
            <p className="text-zinc-500 text-xs">
              Use the same key as <code className="text-zinc-400">VALID_API_KEYS</code> on the gateway
              (plain <code className="text-zinc-400">dev-key</code> works in dev).
            </p>
            <input
              type="password"
              placeholder="API key"
              className="w-full bg-zinc-950 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 font-mono"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
            />
            <button
              type="button"
              disabled={!apiKey.trim()}
              onClick={() => {
                setStoredApiKey(apiKey.trim());
                setMessage('API key saved for this browser');
                setError('');
                navigate('/');
              }}
              className="w-full py-2 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500 disabled:opacity-50"
            >
              Continue with API key
            </button>
          </div>
        )}

        {message && <p className="text-green-400 text-xs">{message}</p>}
        {error && <p className="text-red-400 text-xs break-all">{error}</p>}

        {token && (
          <p className="text-zinc-500 text-xs text-center">
            JWT stored ·{' '}
            <button
              type="button"
              className="text-blue-400 hover:underline"
              onClick={() => {
                clearAuthToken();
                setMessage('Signed out');
              }}
            >
              Sign out
            </button>
          </p>
        )}

        <p className="text-zinc-600 text-xs text-center">
          Dev mode with <code className="text-zinc-500">CLAWZ_DISABLE_AUTH=1</code> skips this screen.
        </p>
      </div>
    </div>
  );
}
