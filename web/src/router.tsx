import { createBrowserRouter } from 'react-router-dom';
import { AuthedLayout } from './components/auth/AuthedLayout';
import { RouteError } from './components/RouteError';
import { Dashboard } from './pages/Dashboard';
import { Agents } from './pages/Agents';
import { Governance } from './pages/Governance';
import { Fleet } from './pages/Fleet';
import { Tools } from './pages/Tools';
import { Config } from './pages/Config';
import { Monitoring } from './pages/Monitoring';
import { Login } from './pages/Login';

export const router = createBrowserRouter([
  {
    path: '/login',
    element: <Login />,
    errorElement: <RouteError />,
  },
  {
    path: '/',
    element: <AuthedLayout />,
    errorElement: <RouteError />,
    children: [
      { index: true, element: <Dashboard /> },
      { path: 'dashboard', element: <Dashboard /> },
      { path: 'agents', element: <Agents /> },
      { path: 'governance', element: <Governance /> },
      { path: 'fleet', element: <Fleet /> },
      { path: 'tools', element: <Tools /> },
      { path: 'config', element: <Config /> },
      { path: 'monitoring', element: <Monitoring /> },
    ],
  },
]);
