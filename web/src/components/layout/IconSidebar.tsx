import { NavLink } from 'react-router-dom';
import { clsx } from 'clsx';
import { ClawzLogo } from '../shared/ClawzLogo';

const NAV_ITEMS = [
  { to: '/', label: 'Dashboard', icon: '⬡', exact: true },
  { to: '/agents', label: 'Agents', icon: '◈' },
  { to: '/governance', label: 'Governance', icon: '⚖' },
  { to: '/fleet', label: 'Fleet', icon: '⬡⬡' },
  { to: '/monitoring', label: 'Monitoring', icon: '◉' },
  { to: '/tools', label: 'Tools', icon: '⚙' },
  { to: '/cron', label: 'Cron', icon: '⏱' },
  { to: '/config', label: 'Config', icon: '≡' },
];

interface IconSidebarProps {
  collapsed: boolean;
  onToggle: () => void;
}

export function IconSidebar({ collapsed, onToggle }: IconSidebarProps) {
  return (
    <aside
      className={clsx(
        'flex flex-col bg-zinc-900 border-r border-zinc-800 transition-all duration-200',
        collapsed ? 'w-12' : 'w-48',
      )}
    >
      <div
        className={clsx(
          'flex border-b border-zinc-800 px-2 py-3',
          collapsed ? 'flex-col items-center gap-1' : 'items-center justify-between',
        )}
      >
        {collapsed ? (
          <ClawzLogo variant="mark" theme="dark" className="h-6 w-6" />
        ) : (
          <ClawzLogo variant="full" theme="dark" className="h-7 w-auto ml-1" />
        )}
        <button
          onClick={onToggle}
          className="p-1.5 rounded-md text-zinc-400 hover:text-zinc-100 hover:bg-zinc-800 transition-colors"
          title={collapsed ? 'Expand' : 'Collapse'}
        >
          {collapsed ? '›' : '‹'}
        </button>
      </div>

      <nav className="flex flex-col gap-0.5 p-1.5 flex-1">
        {NAV_ITEMS.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.exact}
            className={({ isActive }) =>
              clsx(
                'flex items-center gap-2.5 px-2 py-2 rounded-lg text-sm transition-colors',
                isActive
                  ? 'bg-blue-600 text-white'
                  : 'text-zinc-400 hover:text-zinc-100 hover:bg-zinc-800',
              )
            }
            title={collapsed ? item.label : undefined}
          >
            <span className="text-base leading-none flex-shrink-0">{item.icon}</span>
            {!collapsed && <span className="truncate">{item.label}</span>}
          </NavLink>
        ))}
      </nav>

      <div className="p-1.5 border-t border-zinc-800">
        <div
          className={clsx(
            'flex items-center gap-2 px-2 py-1.5 rounded-lg',
            collapsed ? 'justify-center' : '',
          )}
        >
          <ClawzLogo variant="mark" theme="dark" className="h-6 w-6 flex-shrink-0" />
          {!collapsed && (
            <div className="min-w-0">
              <div className="text-xs text-zinc-200 font-medium truncate">ClawZ</div>
              <div className="text-xs text-zinc-500 truncate">v1.0</div>
            </div>
          )}
        </div>
      </div>
    </aside>
  );
}
