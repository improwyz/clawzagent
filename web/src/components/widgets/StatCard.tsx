import { clsx } from 'clsx';

type Variant = 'default' | 'success' | 'warning' | 'error';

const variantAccent: Record<Variant, string> = {
  default: 'text-blue-400',
  success: 'text-green-400',
  warning: 'text-yellow-400',
  error: 'text-red-400',
};

interface StatCardProps {
  label: string;
  value: string | number;
  change?: string;
  trend?: 'up' | 'down' | 'flat';
  variant?: Variant;
  loading?: boolean;
  unit?: string;
}

export function StatCard({
  label,
  value,
  change,
  trend,
  variant = 'default',
  loading = false,
  unit,
}: StatCardProps) {
  const trendIcon = trend === 'up' ? '↑' : trend === 'down' ? '↓' : '→';
  const trendColor = trend === 'up' ? 'text-green-400' : trend === 'down' ? 'text-red-400' : 'text-zinc-500';

  return (
    <div className="widget-3d p-4 rounded-xl bg-zinc-900 border border-zinc-800">
      <div className="text-zinc-400 text-xs font-medium uppercase tracking-wider mb-2">{label}</div>
      {loading ? (
        <div className="h-8 w-20 bg-zinc-800 rounded animate-pulse mb-1" />
      ) : (
        <div className="flex items-baseline gap-1">
          <span className={clsx('text-2xl font-bold font-mono', variantAccent[variant])}>
            {value}
          </span>
          {unit && <span className="text-zinc-500 text-sm">{unit}</span>}
        </div>
      )}
      {change && (
        <div className={clsx('flex items-center gap-1 text-xs mt-1', trendColor)}>
          {trend && <span>{trendIcon}</span>}
          <span>{change}</span>
        </div>
      )}
    </div>
  );
}
