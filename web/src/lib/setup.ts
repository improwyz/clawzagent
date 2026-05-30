import { getApiBase, getStoredApiKey, getStoredAuthToken } from './api';

const SETUP_BOOTSTRAP_TOKEN_KEY = 'clawz_setup_bootstrap_token';

/** Win/Mac/Linux detection for web wizard copy and gateway platform hints. */
export type ClientPlatform = 'windows' | 'macos' | 'linux' | 'other';

export function getSetupBootstrapToken(): string | null {
  return typeof localStorage !== 'undefined'
    ? localStorage.getItem(SETUP_BOOTSTRAP_TOKEN_KEY)
    : null;
}

export function setSetupBootstrapToken(token: string): void {
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem(SETUP_BOOTSTRAP_TOKEN_KEY, token);
  }
}

export function clearSetupBootstrapToken(): void {
  if (typeof localStorage !== 'undefined') {
    localStorage.removeItem(SETUP_BOOTSTRAP_TOKEN_KEY);
  }
}

function persistBootstrapToken(status: Pick<SetupStatus, 'bootstrap_token'>): void {
  if (status.bootstrap_token) {
    setSetupBootstrapToken(status.bootstrap_token);
  }
}

/** Minimal fetch helper for setup routes (bootstrap-friendly). */
async function setupReq<T>(path: string, init?: RequestInit): Promise<T> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...((init?.headers as Record<string, string>) ?? {}),
  };
  const bootstrap = getSetupBootstrapToken();
  if (bootstrap) headers['X-Clawz-Setup-Token'] = bootstrap;
  const token = getStoredAuthToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  else {
    const apiKey =
      getStoredApiKey() || (import.meta.env.VITE_API_KEY as string | undefined);
    if (apiKey) headers['X-API-Key'] = apiKey;
  }

  const res = await fetch(`${getApiBase()}${path}`, { ...init, headers });
  const text = await res.text();
  if (!res.ok) {
    let msg = text || res.statusText;
    try {
      const j = JSON.parse(text) as { error?: string; message?: string };
      msg = j.error ?? j.message ?? msg;
    } catch {
      /* plain text */
    }
    throw new Error(`Setup API ${res.status}: ${msg}`);
  }
  if (!text) return undefined as T;
  return JSON.parse(text) as T;
}

export function isSetupApiUnavailable(err: unknown): boolean {
  if (!(err instanceof Error)) return false;
  return /\bSetup API (404|501|502|503)\b/i.test(err.message);
}

export function isSetupExemptPath(pathname: string): boolean {
  const path = pathname.replace(/\/$/, '') || '/';
  return path === '/setup' || path.startsWith('/setup/') || path === '/login' || path.startsWith('/login/');
}

export function detectClientPlatform(): ClientPlatform {
  if (typeof navigator === 'undefined') return 'other';
  const ua = navigator.userAgent.toLowerCase();
  const platform = (navigator.platform ?? '').toLowerCase();
  if (platform.includes('win') || ua.includes('windows')) return 'windows';
  if (
    platform.includes('mac') ||
    ua.includes('macintosh') ||
    ua.includes('mac os')
  ) {
    return 'macos';
  }
  if (platform.includes('linux') || ua.includes('linux')) return 'linux';
  return 'other';
}

export type SetupPlatform = 'linux' | 'macos' | 'windows' | 'web' | 'mobile' | 'unknown';
export type DeploymentChoice = 'standalone' | 'micro' | 'elastic';
export type InstallStrategy = 'prebuilt' | 'build' | 'source';
export type OAuthProvider = 'cursor' | 'codex' | 'anthropic' | 'openai' | 'skip';

export interface HostSpecReport {
  os: string;
  arch: string;
  ram_mb?: number;
  disk_free_gb?: number;
  cpu_count?: number;
  docker_available: boolean;
  docker_detail?: string;
  compose_v2_available?: boolean;
  compose_detail?: string;
  rust_version?: string;
  rust_meets_minimum?: boolean;
  node_version?: string;
  github_token_present?: boolean;
  summary: string;
  warnings: string[];
}

export interface DoctorCheck {
  name: string;
  ok: boolean;
  detail: string;
  remediation?: string;
}

export interface SetupSecrets {
  jwt_secret?: string;
  jwt_secret_masked?: string;
  api_keys?: string[];
  api_keys_masked?: string[];
  worker_token?: string;
  worker_token_masked?: string;
}

export interface SetupStatus {
  setup_complete: boolean;
  /** Issued once during bootstrap; stored in `clawz_setup_bootstrap_token`. */
  bootstrap_token?: string;
  current_step?: number;
  platform?: SetupPlatform;
  session_id?: string;
  host_spec?: HostSpecReport;
  routes_enabled?: boolean;
}

export interface SetupSessionResponse {
  session_id: string;
  current_step: number;
  platform?: SetupPlatform;
}

export interface SetupAnswerPayload {
  step: number;
  field?: string;
  value?: string | boolean | string[];
  answers?: Record<string, unknown>;
}

export interface SetupAnswerResponse {
  current_step: number;
  host_spec?: HostSpecReport;
  secrets?: SetupSecrets;
  doctor?: { checks: DoctorCheck[]; all_ok?: boolean };
  message?: string;
}

export interface SetupOAuthStartResponse {
  provider: OAuthProvider;
  auth_url?: string;
  device_code?: string;
  message?: string;
}

export const PLACEHOLDER_HOST_SPEC: HostSpecReport = {
  os: 'Unknown host',
  arch: typeof navigator !== 'undefined' ? 'web' : 'unknown',
  docker_available: false,
  summary:
    'Gateway setup API unavailable — connect a gateway or run `clawz onboard` on Linux for a full host probe.',
  warnings: [
    'Could not reach GET /api/v1/setup/status. Start the gateway or set VITE_API_BASE.',
    'Docker and RAM checks will run after the stack is reachable.',
  ],
};

/** Browser platform label for setup API (`SetupPlatform`). */
export function detectWebPlatform(): SetupPlatform {
  switch (detectClientPlatform()) {
    case 'windows':
      return 'windows';
    case 'macos':
      return 'macos';
    case 'linux':
      return 'linux';
    default:
      return 'web';
  }
}

export function isDesktopWebPlatform(): boolean {
  const p = detectWebPlatform();
  return p === 'windows' || p === 'macos';
}

export const SETUP_ROUTE = '/setup';

/** True when the app should send the user to `/setup` (incomplete and not already there). */
export function shouldRedirectToSetup(status: SetupStatus, pathname: string): boolean {
  if (status.setup_complete) return false;
  return !isSetupExemptPath(pathname);
}

/** Redirect target for router guards, or `null` when no redirect is needed. */
export function getSetupRedirectTarget(status: SetupStatus, pathname: string): string | null {
  return shouldRedirectToSetup(status, pathname) ? SETUP_ROUTE : null;
}

export async function fetchSetupStatus(): Promise<SetupStatus> {
  try {
    const status = await setupReq<SetupStatus>('/setup/status');
    persistBootstrapToken(status);
    return status;
  } catch (err) {
    if (isSetupApiUnavailable(err)) {
      throw err;
    }
    return { setup_complete: false, host_spec: PLACEHOLDER_HOST_SPEC, routes_enabled: false };
  }
}

export async function startSetupSession(platform?: SetupPlatform): Promise<SetupSessionResponse> {
  return setupReq<SetupSessionResponse>('/setup/session', {
    method: 'POST',
    body: JSON.stringify({ platform: platform ?? detectWebPlatform(), resume: true }),
  });
}

export async function submitSetupAnswer(payload: SetupAnswerPayload): Promise<SetupAnswerResponse> {
  return setupReq<SetupAnswerResponse>('/setup/answer', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export async function applySetup(step?: number): Promise<SetupAnswerResponse> {
  return setupReq<SetupAnswerResponse>('/setup/apply', {
    method: 'POST',
    body: JSON.stringify(step != null ? { step } : {}),
  });
}

export async function completeSetup(): Promise<{ setup_complete: boolean; dashboard_url?: string }> {
  const result = await setupReq<{ setup_complete: boolean; dashboard_url?: string }>(
    '/setup/complete',
    { method: 'POST', body: '{}' },
  );
  clearSetupBootstrapToken();
  return result;
}

export async function startSetupOAuth(provider: OAuthProvider): Promise<SetupOAuthStartResponse> {
  return setupReq<SetupOAuthStartResponse>('/setup/oauth/start', {
    method: 'POST',
    body: JSON.stringify({ provider }),
  });
}

export async function runSetupDoctor(): Promise<{ checks: DoctorCheck[]; all_ok: boolean }> {
  try {
    const res = await submitSetupAnswer({ step: 9, field: 'run_doctor', value: 'true' });
    const checks = res.doctor?.checks ?? [];
    return { checks, all_ok: res.doctor?.all_ok ?? checks.every((c) => c.ok) };
  } catch {
    return {
      checks: [
        {
          name: 'setup_api',
          ok: false,
          detail: 'Doctor endpoint not available yet.',
          remediation: 'Complete gateway setup routes (section 5) or run `clawz doctor` on the host.',
        },
      ],
      all_ok: false,
    };
  }
}
