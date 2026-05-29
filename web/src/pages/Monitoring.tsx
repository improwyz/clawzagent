import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { clsx } from 'clsx';
import { StatCard } from '../components/widgets/StatCard';
import { Badge } from '../components/shared/Badge';
import { fetchDashboardOverview, type DashboardApiCatalogItem } from '../lib/api';

function formatUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return `${h}h ${m}m`;
}

function methodBadgeVariant(method: string): 'info' | 'success' | 'warning' | 'error' | 'purple' | 'default' {
  switch (method.toUpperCase()) {
    case 'GET':
      return 'info';
    case 'POST':
      return 'success';
    case 'PUT':
    case 'PATCH':
      return 'warning';
    case 'DELETE':
      return 'error';
    case 'WS':
      return 'purple';
    default:
      return 'default';
  }
}

function ApiCatalogRow({ item }: { item: DashboardApiCatalogItem }) {
  return (
    <div className="flex items-start gap-2 py-2 border-b border-zinc-800/80 last:border-0">
      <Badge variant={methodBadgeVariant(item.method)} className="font-mono shrink-0 min-w-[3rem] justify-center">
        {item.method}
      </Badge>
      <code className="text-zinc-300 text-xs font-mono shrink-0">{item.path}</code>
      <span className="text-zinc-500 text-xs flex-1 min-w-0">{item.summary}</span>
    </div>
  );
}

export function Monitoring() {
  const [openGroups, setOpenGroups] = useState<Record<string, boolean>>({});

  const { data, isLoading, error } = useQuery({
    queryKey: ['dashboard-overview'],
    queryFn: fetchDashboardOverview,
    refetchInterval: 30_000,
  });

  const health = data?.health;
  const counts = data?.counts;
  const isHealthy = health?.status === 'healthy';

  const countEntries: { key: keyof NonNullable<typeof counts>; label: string }[] = [
    { key: 'agents', label: 'Agents' },
    { key: 'agents_running', label: 'Agents Running' },
    { key: 'conversations', label: 'Conversations' },
    { key: 'rooms', label: 'Rooms' },
    { key: 'channels', label: 'Channels' },
    { key: 'providers', label: 'Providers' },
    { key: 'tools', label: 'Tools' },
    { key: 'policies', label: 'Policies' },
    { key: 'fleet_nodes', label: 'Fleet Nodes' },
    { key: 'fleet_nodes_online', label: 'Fleet Online' },
    { key: 'deployments', label: 'Deployments' },
    { key: 'audit_entries', label: 'Audit Entries' },
    { key: 'pending_approvals', label: 'Pending Approvals' },
  ];

  const metricsPath = data?.links?.prometheus_metrics ?? '/api/v1/system/metrics';
  const openapiPath = data?.links?.openapi ?? '/api/v1/system/openapi';
  const mcpPath = data?.links?.mcp ?? '/api/v1/mcp';

  const toggleGroup = (group: string) => {
    setOpenGroups((prev) => ({ ...prev, [group]: !prev[group] }));
  };

  return (
    <div className="flex flex-col h-full overflow-y-auto p-4 gap-4">
      <h2 className="text-zinc-100 font-semibold">Monitoring</h2>

      {/* Health banner */}
      <div
        className={clsx(
          'widget-3d rounded-xl border p-4 flex flex-wrap items-center gap-4',
          isHealthy
            ? 'bg-zinc-900 border-green-800/50'
            : 'bg-zinc-900 border-yellow-800/50',
        )}
      >
        <div className="flex items-center gap-3">
          <span
            className={clsx(
              'w-3 h-3 rounded-full',
              isHealthy ? 'bg-green-500 animate-pulse' : 'bg-yellow-500',
            )}
          />
          <div>
            <div className="text-zinc-100 font-medium capitalize">
              {health?.status ?? (isLoading ? 'Loading…' : 'Unknown')}
            </div>
            <div className="text-zinc-500 text-xs">Gateway health</div>
          </div>
        </div>
        <div className="flex flex-wrap gap-6 text-sm">
          <div>
            <span className="text-zinc-500 text-xs uppercase tracking-wider">Uptime</span>
            <div className="text-zinc-200 font-mono">
              {health ? formatUptime(health.uptime_secs) : '—'}
            </div>
          </div>
          <div>
            <span className="text-zinc-500 text-xs uppercase tracking-wider">Version</span>
            <div className="text-zinc-200 font-mono">{health?.version ?? '—'}</div>
          </div>
          <div>
            <span className="text-zinc-500 text-xs uppercase tracking-wider">Auth</span>
            <div className="text-zinc-200">
              {health?.auth_disabled ? 'Disabled (dev)' : 'Enabled'}
            </div>
          </div>
        </div>
        {data?.generated_at && (
          <div className="ml-auto text-zinc-600 text-xs font-mono">
            {new Date(data.generated_at).toLocaleString()}
          </div>
        )}
      </div>

      {error && (
        <div className="px-3 py-2 bg-red-900/30 border border-red-700 rounded text-red-400 text-sm">
          Failed to load overview: {String(error)}
        </div>
      )}

      {/* Quick links */}
      <div className="flex flex-wrap gap-2">
        <a
          href={metricsPath}
          target="_blank"
          rel="noopener noreferrer"
          className="px-3 py-1.5 text-sm rounded-lg bg-zinc-800 border border-zinc-700 text-zinc-300 hover:border-zinc-500 hover:text-zinc-100 transition-colors"
        >
          Prometheus metrics ↗
        </a>
        <a
          href={openapiPath}
          target="_blank"
          rel="noopener noreferrer"
          className="px-3 py-1.5 text-sm rounded-lg bg-zinc-800 border border-zinc-700 text-zinc-300 hover:border-zinc-500 hover:text-zinc-100 transition-colors"
        >
          OpenAPI ↗
        </a>
        <a
          href={mcpPath}
          target="_blank"
          rel="noopener noreferrer"
          className="px-3 py-1.5 text-sm rounded-lg bg-zinc-800 border border-zinc-700 text-zinc-300 hover:border-zinc-500 hover:text-zinc-100 transition-colors"
        >
          MCP endpoint ↗
        </a>
      </div>

      {/* Resource counts */}
      <div>
        <h3 className="text-zinc-400 text-xs font-medium uppercase tracking-wider mb-3">
          Resource counts
        </h3>
        <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-3">
          {countEntries.map(({ key, label }) => (
            <StatCard
              key={key}
              label={label}
              value={counts?.[key] ?? '—'}
              loading={isLoading}
              variant={
                key === 'pending_approvals' && (counts?.pending_approvals ?? 0) > 0
                  ? 'warning'
                  : key === 'agents_running' && (counts?.agents_running ?? 0) > 0
                    ? 'success'
                    : 'default'
              }
            />
          ))}
        </div>
      </div>

      {/* API catalog */}
      <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800 overflow-hidden flex-1 min-h-0">
        <div className="px-4 py-3 border-b border-zinc-800">
          <h3 className="text-zinc-300 text-xs font-medium uppercase tracking-wider">
            API catalog
          </h3>
          <p className="text-zinc-500 text-xs mt-1">
            Gateway routes grouped by domain
          </p>
        </div>
        <div className="divide-y divide-zinc-800 max-h-[480px] overflow-y-auto">
          {isLoading &&
            Array.from({ length: 4 }).map((_, i) => (
              <div key={i} className="px-4 py-3 h-12 bg-zinc-800/30 animate-pulse" />
            ))}
          {!isLoading &&
            (data?.api_catalog ?? []).map((group) => {
              const open = openGroups[group.group] ?? false;
              return (
                <div key={group.group}>
                  <button
                    type="button"
                    onClick={() => toggleGroup(group.group)}
                    className="w-full flex items-center justify-between px-4 py-3 text-left hover:bg-zinc-800/50 transition-colors"
                  >
                    <span className="text-zinc-200 text-sm font-medium">{group.group}</span>
                    <span className="flex items-center gap-2 text-zinc-500 text-xs">
                      <span>{group.items.length} endpoints</span>
                      <span className="text-zinc-600">{open ? '▼' : '▶'}</span>
                    </span>
                  </button>
                  {open && (
                    <div className="px-4 pb-3 bg-zinc-950/50">
                      {group.items.map((item, idx) => (
                        <ApiCatalogRow key={`${item.method}-${item.path}-${idx}`} item={item} />
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
        </div>
      </div>
    </div>
  );
}
