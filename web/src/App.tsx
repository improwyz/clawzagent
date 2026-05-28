import { Routes, Route } from 'react-router-dom';
import { AuthGate } from './components/auth/AuthGate';
import { Shell } from './components/layout/Shell';
import { Dashboard } from './pages/Dashboard';
import { Agents } from './pages/Agents';
import { Governance } from './pages/Governance';
import { Fleet } from './pages/Fleet';
import { Tools } from './pages/Tools';
import { Config } from './pages/Config';
import { Login } from './pages/Login';

export default function App() {
  return (
    <Routes>
      <Route path="/login" element={<Login />} />
      <Route
        path="/*"
        element={
          <AuthGate>
            <Shell>
              <Routes>
                <Route path="/" element={<Dashboard />} />
                <Route path="/dashboard" element={<Dashboard />} />
                <Route path="/agents" element={<Agents />} />
                <Route path="/governance" element={<Governance />} />
                <Route path="/fleet" element={<Fleet />} />
                <Route path="/tools" element={<Tools />} />
                <Route path="/config" element={<Config />} />
              </Routes>
            </Shell>
          </AuthGate>
        }
      />
    </Routes>
  );
}