import { useEffect, useState } from 'react';
import { Navigate, Outlet, useLocation } from 'react-router-dom';
import {
  fetchSetupStatus,
  getSetupRedirectTarget,
  isSetupApiUnavailable,
} from '../../lib/setup';

/**
 * Redirects to `/setup` when first-run onboarding is incomplete.
 * Wraps authenticated dashboard routes (inside AuthedLayout).
 */
export function SetupGuard() {
  const location = useLocation();
  const [checking, setChecking] = useState(true);
  const [redirectTo, setRedirectTo] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setChecking(true);
    fetchSetupStatus()
      .then((status) => {
        if (!cancelled) {
          setRedirectTo(getSetupRedirectTarget(status, location.pathname));
        }
      })
      .catch((err) => {
        if (!cancelled) {
          if (isSetupApiUnavailable(err)) {
            setRedirectTo(null);
          } else {
            console.warn('setup status check failed:', err);
            setRedirectTo('/setup');
          }
        }
      })
      .finally(() => {
        if (!cancelled) setChecking(false);
      });
    return () => {
      cancelled = true;
    };
  }, [location.pathname]);

  if (checking) {
    return (
      <div className="flex min-h-[40vh] items-center justify-center text-zinc-400 text-sm">
        <div
          className="mr-3 h-8 w-8 rounded-full border-2 border-zinc-700 border-t-blue-500 animate-spin"
          aria-hidden
        />
        Checking setup status…
      </div>
    );
  }

  if (redirectTo) {
    return <Navigate to={redirectTo} replace state={{ from: location.pathname }} />;
  }

  return <Outlet />;
}
