import { NavLink } from 'react-router-dom';

const MOBILE_LINKS = [
  { to: '/', label: 'Home', icon: '🏠' },
  { to: '/agents', label: 'Agents', icon: '🤖' },
  { to: '/cron', label: 'Cron', icon: '⏱' },
  { to: '/config', label: 'Config', icon: '⚙️' },
] as const;

export function MobileNav() {
  return (
    <nav
      className="md:hidden fixed bottom-0 inset-x-0 z-50 flex items-stretch justify-around border-t border-zinc-800 bg-zinc-900/95 backdrop-blur pb-[env(safe-area-inset-bottom)]"
      aria-label="Mobile navigation"
    >
      {MOBILE_LINKS.map((link) => (
        <NavLink
          key={link.to}
          to={link.to}
          end={link.to === '/'}
          className={({ isActive }) =>
            `flex flex-1 flex-col items-center justify-center gap-0.5 py-2 text-[10px] ${
              isActive ? 'text-blue-400' : 'text-zinc-500'
            }`
          }
        >
          <span className="text-base leading-none" aria-hidden>
            {link.icon}
          </span>
          <span>{link.label}</span>
        </NavLink>
      ))}
    </nav>
  );
}
