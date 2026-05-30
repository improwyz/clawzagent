import { useEffect, useState, type ReactNode } from 'react';
import { initShellApi } from '../lib/api';
import { isTauriShell } from '../lib/shell';
import {
  fetchSetupStatus,
  isSetupApiUnavailable,
  isSetupExemptPath,
} from '../lib/setup';

/** Loads Tauri shell config and setup status before the dashboard fetches. */
export function ShellBootstrap({ children }: { children: ReactNode }) {
  const exempt = isSetupExemptPath(window.location.pathname);
  const [shellReady, setShellReady] = useState(!isTauriShell());
  const [setupReady, setSetupReady] = useState(exempt);

  useEffect(() => {
    if (!isTauriShell()) return;
    initShellApi()
      .catch((e) => console.warn('shell init:', e))
      .finally(() => setShellReady(true));
  }, []);

  useEffect(() => {
    if (exempt) return;
    let cancelled = false;
    fetchSetupStatus()
      .catch((err) => {
        if (!isSetupApiUnavailable(err)) {
          console.warn('setup status (bootstrap):', err);
        }
      })
      .finally(() => {
        if (!cancelled) setSetupReady(true);
      });
    return () => {
      cancelled = true;
    };
  }, [exempt]);

  if (!shellReady || !setupReady) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-zinc-950 text-zinc-400 text-sm">
        {!shellReady ? 'Loading desktop shell…' : 'Checking setup status…'}
      </div>
    );
  }

  return <>{children}</>;
}
