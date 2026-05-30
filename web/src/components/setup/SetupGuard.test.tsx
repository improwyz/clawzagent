import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SetupGuard } from './SetupGuard';
import * as setup from '../../lib/setup';

vi.mock('../../lib/setup', async (importOriginal) => {
  const mod = await importOriginal<typeof setup>();
  return { ...mod, fetchSetupStatus: vi.fn() };
});

function renderGuard(initialPath: string) {
  return render(
    <MemoryRouter initialEntries={[initialPath]}>
      <Routes>
        <Route path="/setup" element={<div>setup wizard</div>} />
        <Route path="/dashboard" element={<SetupGuard />}>
          <Route index element={<div>dashboard</div>} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );
}

describe('SetupGuard', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('redirects to /setup when setup is incomplete', async () => {
    vi.mocked(setup.fetchSetupStatus).mockResolvedValue({
      setup_complete: false,
      step: 'welcome',
    });

    renderGuard('/dashboard');

    await waitFor(() => {
      expect(screen.getByText('setup wizard')).toBeInTheDocument();
    });
  });

  it('renders outlet when setup is complete', async () => {
    vi.mocked(setup.fetchSetupStatus).mockResolvedValue({
      setup_complete: true,
      step: 'complete',
    });

    renderGuard('/dashboard');

    await waitFor(() => {
      expect(screen.getByText('dashboard')).toBeInTheDocument();
    });
  });
});
