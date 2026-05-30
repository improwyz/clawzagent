import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  fetchSetupStatus,
  getSetupRedirectTarget,
  PLACEHOLDER_HOST_SPEC,
  shouldRedirectToSetup,
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
