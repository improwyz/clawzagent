import { useEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { StatCard } from '../components/widgets/StatCard';
import { MessagesBarChart } from '../components/widgets/BarChart';
import { DonutChart } from '../components/widgets/DonutChart';
import { FleetTable } from '../components/widgets/FleetTable';
import { ChannelGrid } from '../components/widgets/ChannelGrid';
import { GovernancePanel } from '../components/widgets/GovernancePanel';
import {
  fetchMetrics,
  fetchFleet,
  fetchChannels,
  fetchGovernance,
  fetchDashboardOverview,
} from '../lib/api';
import { Badge } from '../components/shared/Badge';
import { connectMetrics } from '../lib/ws';

interface LiveMetrics {
  active_agents?: number;
  total_conversations?: number;
  requests_per_min?: number;
  avg_latency_ms?: number;
}

export function Dashboard() {
  const [live, setLive] = useState<LiveMetrics>({});
  const wsRef = useRef<WebSocket | null>(null);

  const { data: metrics, isLoading: metricsLoading } = useQuery({
    queryKey: ['metrics'],
    queryFn: fetchMetrics,
    refetchInterval: 30_000,
  });

  const { data: fleet, isLoading: fleetLoading } = useQuery({
    queryKey: ['fleet'],
    queryFn: fetchFleet,
    refetchInterval: 30_000,
  });

  const { data: channels, isLoading: channelsLoading } = useQuery({
    queryKey: ['channels'],
    queryFn: fetchChannels,
    refetchInterval: 30_000,
  });

  const { data: governance, isLoading: govLoading } = useQuery({
    queryKey: ['governance'],
    queryFn: fetchGovernance,
    refetchInterval: 60_000,
  });

  const { data: overview, isLoading: overviewLoading } = useQuery({
    queryKey: ['dashboard-overview'],
    queryFn: fetchDashboardOverview,
    refetchInterval: 30_000,
  });

  useEffect(() => {
    wsRef.current = connectMetrics((data) => {
      setLive((prev) => ({ ...prev, ...(data as LiveMetrics) }));
    });
    return () => wsRef.current?.close();
  }, []);

  // REST snapshot is source of truth; WS only fills gaps while the query is loading.
  const activeAgents = metrics?.active_agents ?? live.active_agents;
  const totalConvos = metrics?.total_conversations ?? live.total_conversations;
  const reqPerMin = metrics?.requests_per_min ?? live.requests_per_min;
  const avgLatency = metrics?.avg_latency_ms ?? live.avg_latency_ms;

  const chartData = metrics?.requests_over_time ?? [
    { time: '00:00', count: 0 },
    { time: '02:00', count: 0 },
    { time: '04:00', count: 0 },
    { time: '06:00', count: 0 },
    { time: '08:00', count: 0 },
    { time: '10:00', count: 0 },
    { time: '12:00', count: 0 },
  ];

  const providerData = metrics?.provider_distribution ?? [];

  const overviewConversations = overview?.counts.conversations;
  const overviewPending = overview?.counts.pending_approvals;

  return (
    <div className="flex flex-col h-full overflow-y-auto p-4 gap-4">
      {/* Health strip from overview */}
      {overview && (
        <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800 px-4 py-2.5 flex flex-wrap items-center gap-3 text-sm">
          <Badge variant={overview.health.status === 'healthy' ? 'success' : 'warning'}>
            {overview.health.status}
          </Badge>
          <span className="text-zinc-400 font-mono text-xs">v{overview.health.version}</span>
          <span className="text-zinc-500 text-xs">
            uptime{' '}
            <span className="text-zinc-300 font-mono">
              {Math.floor(overview.health.uptime_secs / 60)}m
            </span>
          </span>
          {overview.health.auth_disabled && (
            <Badge variant="warning">auth off</Badge>
          )}
        </div>
      )}

      {/* KPI row */}
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3">
        <StatCard
          label="Active Agents"
          value={activeAgents ?? '—'}
          loading={metricsLoading && activeAgents === undefined}
          variant={activeAgents ? 'success' : 'default'}
        />
        <StatCard
          label="Total Conversations"
          value={totalConvos !== undefined ? totalConvos.toLocaleString() : '—'}
          loading={metricsLoading && totalConvos === undefined}
        />
        <StatCard
          label="Requests / min"
          value={reqPerMin ?? '—'}
          loading={metricsLoading && reqPerMin === undefined}
          variant={reqPerMin !== undefined && reqPerMin > 100 ? 'warning' : 'default'}
        />
        <StatCard
          label="Avg Latency"
          value={avgLatency ?? '—'}
          unit="ms"
          loading={metricsLoading && avgLatency === undefined}
          variant={avgLatency !== undefined && avgLatency > 500 ? 'error' : 'default'}
        />
        <StatCard
          label="Conversations"
          value={
            overviewConversations !== undefined
              ? overviewConversations.toLocaleString()
              : totalConvos !== undefined
                ? totalConvos.toLocaleString()
                : '—'
          }
          loading={overviewLoading && metricsLoading}
        />
        <StatCard
          label="Pending Approvals"
          value={overviewPending ?? governance?.approvals?.filter((a) => a.status === 'pending').length ?? '—'}
          loading={overviewLoading && govLoading}
          variant={(overviewPending ?? 0) > 0 ? 'warning' : 'default'}
        />
      </div>

      {/* Charts row */}
      <div className="grid grid-cols-2 gap-3">
        <div className="widget-3d p-4 rounded-xl bg-zinc-900 border border-zinc-800">
          <h3 className="text-zinc-300 text-xs font-medium uppercase tracking-wider mb-3">
            Requests Over Time
          </h3>
          <MessagesBarChart
            data={chartData}
            dataKey="count"
            xKey="time"
            loading={metricsLoading}
          />
        </div>
        <div className="widget-3d p-4 rounded-xl bg-zinc-900 border border-zinc-800">
          <h3 className="text-zinc-300 text-xs font-medium uppercase tracking-wider mb-3">
            Provider Distribution
          </h3>
          {providerData.length === 0 && !metricsLoading ? (
            <div className="h-52 flex items-center justify-center text-zinc-500 text-sm">
              No provider data yet
            </div>
          ) : (
            <DonutChart data={providerData} loading={metricsLoading} />
          )}
        </div>
      </div>

      {/* Fleet + Channels row */}
      <div className="grid grid-cols-2 gap-3">
        <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800 overflow-hidden">
          <div className="px-4 py-3 border-b border-zinc-800 flex items-center justify-between">
            <h3 className="text-zinc-300 text-xs font-medium uppercase tracking-wider">Fleet Nodes</h3>
            <span className="text-zinc-500 text-xs font-mono">
              {fleet?.nodes?.length ?? 0} nodes
            </span>
          </div>
          <FleetTable nodes={fleet?.nodes ?? []} loading={fleetLoading} />
        </div>

        <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800">
          <div className="px-4 py-3 border-b border-zinc-800 flex items-center justify-between">
            <h3 className="text-zinc-300 text-xs font-medium uppercase tracking-wider">Channels</h3>
            <span className="text-zinc-500 text-xs font-mono">
              {channels?.filter((c) => c.status === 'active').length ?? 0} active
            </span>
          </div>
          <div className="p-3">
            <ChannelGrid channels={channels ?? []} loading={channelsLoading} />
          </div>
        </div>
      </div>

      {/* Governance row */}
      <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800">
        <div className="px-4 py-3 border-b border-zinc-800">
          <h3 className="text-zinc-300 text-xs font-medium uppercase tracking-wider">
            Governance — Approval Queue
          </h3>
        </div>
        <div className="p-4">
          <GovernancePanel
            approvals={governance?.approvals ?? []}
            loading={govLoading}
          />
        </div>
      </div>
    </div>
  );
}
