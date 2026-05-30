/** Desktop Tauri shell bridge — gateway URL, mode, keyring-backed API key. */

export type ShellMode = 'standalone' | 'gateway';

export interface ShellConfig {
  mode: ShellMode;
  gateway_url?: string | null;
  api_key_set: boolean;
  connection_status: string;
}

export interface ShellConfigInput {
  mode: ShellMode;
  gateway_url?: string;
  /** Empty string clears the stored key. */
  api_key?: string;
}

export function isTauriShell(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: tauriInvoke } = await import('@tauri-apps/api/core');
  return tauriInvoke<T>(cmd, args);
}

export async function getShellConfig(): Promise<ShellConfig> {
  return invoke<ShellConfig>('get_shell_config');
}

export async function setShellConfig(input: ShellConfigInput): Promise<ShellConfig> {
  return invoke<ShellConfig>('set_shell_config', {
    input: {
      mode: input.mode,
      gateway_url: input.gateway_url ?? null,
      api_key: input.api_key ?? null,
    },
  });
}

export async function loadGatewayApiKey(): Promise<string | null> {
  const key = await invoke<string | null>('load_gateway_api_key');
  return key && key.length > 0 ? key : null;
}
