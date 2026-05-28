import { useEffect } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Navigate, useLocation } from 'react-router-dom';
import {
  fetchAuthStatus,
  hasAuthCredentials,
  setAuthFailureHandler,
  clearAuthToken,
} from '../../lib/api';

/**
 * Never unmounts children — only overlays while auth status loads.
 * Unmounting the layout broke <Outlet /> updates on client-side navigation.
 */
export function AuthGate({ children }: { children: React.ReactNode }) {
  const location = useLocation();
  const { data: status, isPending, isError } = useQuery({
    queryKey: ['auth-status'],
    queryFn: fetchAuthStatus,
    staleTime: 60_000,
    gcTime: 300_000,
    retry: 1,
  });

  useEffect(() => {
    setAuthFailureHandler(() => {
      clearAuthToken();
      window.location.href = '/login';
    });
    return () => setAuthFailureHandler(null);
  }, []);

  const authDisabled = isError || status?.auth_disabled === true;

  if (!isPending && !authDisabled && !hasAuthCredentials()) {
    return <Navigate to="/login" replace state={{ from: location.pathname }} />;
  }

  return (
    <div className="relative h-screen w-screen overflow-hidden">
      {isPending && (
        <div className="pointer-events-none absolute inset-0 z-50 flex items-center justify-center bg-zinc-950/80 text-zinc-400 text-sm">
          Checking authentication…
        </div>
      )}
      {children}
    </div>
  );
}
