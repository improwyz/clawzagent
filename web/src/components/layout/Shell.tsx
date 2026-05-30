import { useState } from 'react';
import { IconSidebar } from './IconSidebar';
import { ChatSidebar } from './ChatSidebar';
import { ClawzLogo } from '../shared/ClawzLogo';
import { ThemeToggle } from '../shared/ThemeToggle';
import { useLocation } from 'react-router-dom';
import { useAppStore } from '../../lib/store';
import { MobileNav } from './MobileNav';

const PAGE_TITLES: Record<string, string> = {
  '/': 'Dashboard',
  '/dashboard': 'Dashboard',
  '/agents': 'Agents',
  '/governance': 'Governance',
  '/fleet': 'Fleet',
  '/tools': 'Tools',
  '/config': 'Config',
};

export function Shell({ children }: { children: React.ReactNode }) {
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [chatWidth, setChatWidth] = useState(300);
  const [chatVisible, setChatVisible] = useState(true);
  const location = useLocation();
  const { notifications, dismissNotification } = useAppStore();
  const title = PAGE_TITLES[location.pathname] ?? 'ClawZ';

  return (
    <div className="flex h-screen bg-zinc-950 text-zinc-100 pb-14 md:pb-0">
      <div className="hidden md:flex">
        <IconSidebar
          collapsed={sidebarCollapsed}
          onToggle={() => setSidebarCollapsed(!sidebarCollapsed)}
        />
      </div>

      <div className="flex flex-col flex-1 min-w-0">
        {/* Top bar */}
        <header className="flex items-center justify-between px-4 py-2 bg-zinc-900 border-b border-zinc-800 flex-shrink-0">
          <div className="flex items-center gap-2">
            <ClawzLogo
              variant="mark"
              theme="dark"
              className="h-5 w-5 md:hidden flex-shrink-0"
            />
            <h1 className="text-zinc-100 font-semibold text-sm">{title}</h1>
          </div>
          <div className="flex items-center gap-2">
            {notifications.length > 0 && (
              <div className="relative">
                <button className="p-1.5 rounded-md text-zinc-400 hover:text-zinc-100 hover:bg-zinc-800">
                  🔔
                </button>
                <span className="absolute -top-1 -right-1 w-4 h-4 bg-red-500 text-white text-xs rounded-full flex items-center justify-center">
                  {notifications.length}
                </span>
              </div>
            )}
            <ThemeToggle />
            <button
              onClick={() => setChatVisible(!chatVisible)}
              className="p-1.5 rounded-md text-zinc-400 hover:text-zinc-100 hover:bg-zinc-800 text-sm transition-colors"
              title={chatVisible ? 'Hide chat' : 'Show chat'}
            >
              {chatVisible ? '»' : '«'}
            </button>
          </div>
        </header>

        {/* Notification bar */}
        {notifications.map((n) => (
          <div
            key={n.id}
            className={`px-4 py-2 text-sm flex items-center justify-between ${
              n.type === 'error' ? 'bg-red-900/50 text-red-300' :
              n.type === 'success' ? 'bg-green-900/50 text-green-300' :
              n.type === 'warning' ? 'bg-yellow-900/50 text-yellow-300' :
              'bg-blue-900/50 text-blue-300'
            }`}
          >
            <span>{n.message}</span>
            <button onClick={() => dismissNotification(n.id)} className="ml-4 hover:opacity-70">✕</button>
          </div>
        ))}

        <main className="flex-1 overflow-hidden min-h-0">
          {children}
        </main>
      </div>

      {chatVisible && (
        <div className="hidden lg:flex">
          <ChatSidebar width={chatWidth} onResize={setChatWidth} />
        </div>
      )}
      <MobileNav />
    </div>
  );
}
