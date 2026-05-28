import { Outlet } from 'react-router-dom';
import { AuthGate } from './AuthGate';
import { Shell } from '../layout/Shell';

/** Layout route: auth check + chrome; child routes render in Shell via Outlet. */
export function AuthedLayout() {
  return (
    <AuthGate>
      <Shell>
        <Outlet />
      </Shell>
    </AuthGate>
  );
}
