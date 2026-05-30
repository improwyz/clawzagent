import { useEffect, useState, type ReactNode } from 'react';
import { initShellApi } from '../lib/api';
import { isTauriShell } from '../lib/shell';

/** Loads Tauri shell config (gateway URL, keyring API key) before the dashboard fetches. */
export function ShellBootstrap({ children }: { children: ReactNode }) {
  const [ready, setReady] = useState(!isTauriShell());

  useEffect(() => {
    if (!isTauriShell()) return;
    initShellApi()
      .catch((e) => console.warn('shell init:', e))
      .finally(() => setReady(true));
  }, []);

  if (!ready) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-zinc-950 text-zinc-400 text-sm">
        Loading desktop shell…
      </div>
    );
  }

  return <>{children}</>;
}
