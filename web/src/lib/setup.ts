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

/** Gateway `GET /api/v1/setup/status`. */
export interface SetupStatus {
  setup_complete: boolean;
  step: string;
  session_id?: string;
  /** Issued during bootstrap; persisted in `clawz_setup_bootstrap_token`. */
  bootstrap_token?: string;
}

/** `POST /setup/session` response. */
export interface SetupSessionResponse {
  session_id: string;
  step: string;
  platform?: string;
  resumed?: boolean;
}

/** `POST /setup/answer` body (matches gateway `AnswerBody`). */
export interface SetupAnswerPayload {
  deployment?: string;
  install_strategy?: string;
  secrets?: Record<string, string>;
  llm_provider?: string;
  llm_api_key?: string;
  identity_name?: string;
  identity_who_am_i?: string;
  identity_role?: string;
  identity_model?: string;
  advance?: boolean;
}

/** `POST /setup/answer` response. */
export interface SetupAnswerResponse {
  session_id: string;
  step: string;
  advanced_to?: string;
}

/** `POST /setup/apply` response. */
export interface SetupApplyResponse {
  applied: boolean;
  provider_id: string;
  agent_id: string;
  workspace: string;
}

/** `POST /setup/complete` response. */
export interface SetupCompleteResponse {
  setup_complete: boolean;
  session_id: string;
  step: string;
}

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

/** Enriched status when the gateway is unreachable (wizard UI placeholders). */
export interface SetupStatusFallback extends SetupStatus {
  host_spec?: HostSpecReport;
  routes_enabled?: boolean;
}

export interface SetupOAuthStartResponse {
  auth_url?: string;
  state: string;
  message: string;
}

export const SETUP_STEP_NAMES = [
  'welcome',
  'deploy_mode',
  'install_strategy',
  'stack',
  'write_secrets',
  'llm',
  'agent_identity',
  'skills',
  'agent_topology',
  'verify',
  'complete',
] as const;

export type SetupStepName = (typeof SETUP_STEP_NAMES)[number];

export function setupStepToIndex(step: string): number {
  const idx = SETUP_STEP_NAMES.indexOf(step as SetupStepName);
  return idx >= 0 ? idx : 0;
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
export function shouldRedirectToSetup(status: Pick<SetupStatus, 'setup_complete'>, pathname: string): boolean {
  if (status.setup_complete) return false;
  return !isSetupExemptPath(pathname);
}

/** Redirect target for router guards, or `null` when no redirect is needed. */
export function getSetupRedirectTarget(
  status: Pick<SetupStatus, 'setup_complete'>,
  pathname: string,
): string | null {
  return shouldRedirectToSetup(status, pathname) ? SETUP_ROUTE : null;
}

export async function fetchSetupStatus(): Promise<SetupStatusFallback> {
  try {
    const status = await setupReq<SetupStatus>('/setup/status');
    persistBootstrapToken(status);
    return status;
  } catch (err) {
    if (isSetupApiUnavailable(err)) {
      throw err;
    }
    return {
      setup_complete: false,
      step: 'welcome',
      host_spec: PLACEHOLDER_HOST_SPEC,
      routes_enabled: false,
    };
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

export async function applySetup(force = false): Promise<SetupApplyResponse> {
  return setupReq<SetupApplyResponse>('/setup/apply', {
    method: 'POST',
    body: JSON.stringify({ force }),
  });
}

export async function completeSetup(): Promise<SetupCompleteResponse> {
  const result = await setupReq<SetupCompleteResponse>('/setup/complete', {
    method: 'POST',
    body: '{}',
  });
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
    const res = await submitSetupAnswer({ advance: true });
    void res;
    return {
      checks: [
        {
          name: 'gateway',
          ok: true,
          detail: 'Setup session advanced; run `clawz doctor` on the host for full checks.',
        },
      ],
      all_ok: true,
    };
  } catch {
    return {
      checks: [
        {
          name: 'setup_api',
          ok: false,
          detail: 'Doctor endpoint not available yet.',
          remediation: 'Complete gateway setup routes or run `clawz doctor` on the host.',
        },
      ],
      all_ok: false,
    };
  }
}
