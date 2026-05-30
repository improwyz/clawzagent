import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  fetchSetupStackStatus,
  fetchSetupStatus,
  getSetupRedirectTarget,
  PLACEHOLDER_HOST_SPEC,
  runSetupStack,
  shouldRedirectToSetup,
  suggestedHostInstallCommand,
} from './setup';

function mockFetchResponse(body: unknown, ok = true, status = ok ? 200 : 503) {
  return {
    ok,
    status,
    statusText: ok ? 'OK' : 'Error',
    text: async () => JSON.stringify(body),
  } as Response;
}

describe('fetchSetupStatus', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
    localStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('calls GET /api/v1/setup/status and returns setup_complete', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      mockFetchResponse({ setup_complete: true, step: 'complete' }),
    );

    const status = await fetchSetupStatus();

    expect(status.setup_complete).toBe(true);
    expect(fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/v1\/setup\/status$/),
      expect.objectContaining({ headers: expect.any(Object) }),
    );
  });

  it('persists bootstrap_token from status into localStorage', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      mockFetchResponse({
        setup_complete: false,
        step: 'welcome',
        bootstrap_token: 'tok-abc',
      }),
    );

    await fetchSetupStatus();

    expect(localStorage.getItem('clawz_setup_bootstrap_token')).toBe('tok-abc');
  });

  it('rethrows when setup API responds unavailable (503)', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      mockFetchResponse({ error: 'unavailable' }, false, 503),
    );

    await expect(fetchSetupStatus()).rejects.toThrow('Setup API 503');
  });

  it('falls back to incomplete placeholder on unexpected errors', async () => {
    vi.mocked(fetch).mockRejectedValueOnce(new Error('network down'));

    const status = await fetchSetupStatus();

    expect(status.setup_complete).toBe(false);
    expect(status.host_spec?.warnings).toEqual(PLACEHOLDER_HOST_SPEC.warnings);
    expect(status.routes_enabled).toBe(false);
  });
});

describe('setup stack API', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
    localStorage.setItem('clawz_setup_bootstrap_token', 'tok-stack');
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('fetchSetupStackStatus calls GET /setup/stack/status', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      mockFetchResponse({
        host_exec_allowed: true,
        suggested_command: './scripts/install.sh --docker',
        docker_available: true,
        compose_v2_available: true,
      }),
    );

    const status = await fetchSetupStackStatus();

    expect(status.host_exec_allowed).toBe(true);
    expect(fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/v1\/setup\/stack\/status$/),
      expect.any(Object),
    );
  });

  it('runSetupStack posts action with confirm token header', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(
      mockFetchResponse({
        ok: true,
        host_exec_allowed: true,
        message: 'bash setup-host-exec.sh ensure_docker',
        dry_run: true,
      }),
    );

    const res = await runSetupStack({ action: 'deps', dry_run: true });

    expect(res.ok).toBe(true);
    const [, init] = vi.mocked(fetch).mock.calls[0] as [string, RequestInit];
    expect(init.method).toBe('POST');
    expect((init.headers as Record<string, string>)['X-Clawz-Setup-Token']).toBe('tok-stack');
    const body = JSON.parse(init.body as string) as { action: string; confirm: string };
    expect(body.action).toBe('deps');
    expect(body.confirm).toBe('yes-install');
  });

  it('suggestedHostInstallCommand includes platform hints', () => {
    expect(suggestedHostInstallCommand('windows')).toContain('install.ps1');
    expect(suggestedHostInstallCommand('linux')).toContain('install.sh');
  });
});

describe('setup redirect helpers', () => {
  it('redirects when setup is incomplete outside exempt routes', () => {
    const incomplete = { setup_complete: false };

    expect(shouldRedirectToSetup(incomplete, '/')).toBe(true);
    expect(shouldRedirectToSetup(incomplete, '/dashboard')).toBe(true);
    expect(getSetupRedirectTarget(incomplete, '/agents')).toBe('/setup');
  });

  it('does not redirect when setup is complete', () => {
    const complete = { setup_complete: true };

    expect(shouldRedirectToSetup(complete, '/')).toBe(false);
    expect(getSetupRedirectTarget(complete, '/dashboard')).toBeNull();
  });

  it('does not redirect on /setup or /login', () => {
    const incomplete = { setup_complete: false };

    expect(shouldRedirectToSetup(incomplete, '/setup')).toBe(false);
    expect(shouldRedirectToSetup(incomplete, '/login')).toBe(false);
    expect(getSetupRedirectTarget(incomplete, '/setup/oauth')).toBeNull();
  });
});
