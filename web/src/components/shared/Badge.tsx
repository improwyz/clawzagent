import { clsx } from 'clsx';

type BadgeVariant = 'default' | 'success' | 'warning' | 'error' | 'info' | 'purple';

const variantClasses: Record<BadgeVariant, string> = {
  default: 'bg-zinc-700 text-zinc-300',
  success: 'bg-green-500/20 text-green-400',
  warning: 'bg-yellow-500/20 text-yellow-400',
  error: 'bg-red-500/20 text-red-400',
  info: 'bg-blue-500/20 text-blue-400',
  purple: 'bg-purple-500/20 text-purple-400',
};

interface BadgeProps {
  children: React.ReactNode;
  variant?: BadgeVariant;
  className?: string;
}

export function Badge({ children, variant = 'default', className }: BadgeProps) {
  return (
    <span
      className={clsx(
        'inline-flex items-center px-2 py-0.5 rounded text-xs font-medium',
        variantClasses[variant],
        className,
      )}
    >
      {children}
    </span>
  );
}

export function statusVariant(
  status: string,
): BadgeVariant {
  switch (status.toLowerCase()) {
    case 'active':
    case 'online':
    case 'running':
    case 'connected':
    case 'approved':
    case 'pass':
      return 'success';
    case 'idle':
    case 'warn':
    case 'warning':
    case 'pending':
    case 'busy':
    case 'flagged':
      return 'warning';
    case 'error':
    case 'offline':
    case 'stopped':
    case 'failed':
    case 'rejected':
    case 'fail':
    case 'blocked':
    case 'restricted':
      return 'error';
    case 'info':
    case 'disconnected':
      return 'info';
    case 'platinum':
    case 'gold':
      return 'purple';
    default:
      return 'default';
  }
}
