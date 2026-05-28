import { useEffect } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Navigate, useLocation } from 'react-router-dom';
import {
  fetchAuthStatus,
  hasAuthCredentials,
  setAuthFailureHandler,
  clearAuthToken,
} from '../../lib/api';

export function AuthGate({ children }: { children: React.ReactNode }) {
  const location = useLocation();
  const { data: status, isLoading } = useQuery({
    queryKey: ['auth-status'],
    queryFn: fetchAuthStatus,
    staleTime: 60_000,
  });

  useEffect(() => {
    setAuthFailureHandler(() => {
      clearAuthToken();
      window.location.href = '/login';
    });
    return () => setAuthFailureHandler(null);
  }, []);

  if (isLoading) {
    return (
      <div className="flex h-screen items-center justify-center bg-zinc-950 text-zinc-400 text-sm">
        Checking authentication…
      </div>
    );
  }

  if (status?.auth_disabled) {
    return <>{children}</>;
  }

  if (!hasAuthCredentials()) {
    return <Navigate to="/login" replace state={{ from: location.pathname }} />;
  }

  return <>{children}</>;
}
