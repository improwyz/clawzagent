import { Badge, statusVariant } from '../shared/Badge';
import type { FleetNode } from '../../lib/api';

interface FleetTableProps {
  nodes: FleetNode[];
  loading?: boolean;
}

function LoadRow() {
  return (
    <tr>
      {[1, 2, 3, 4, 5, 6].map((i) => (
        <td key={i} className="px-3 py-2.5">
          <div className="h-3 bg-zinc-800 rounded animate-pulse w-3/4" />
        </td>
      ))}
    </tr>
  );
}

export function FleetTable({ nodes, loading = false }: FleetTableProps) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-zinc-700">
            <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Hostname</th>
            <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Status</th>
            <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Agents</th>
            <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">CPU</th>
            <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Mem</th>
            <th className="text-left text-zinc-400 text-xs font-medium px-3 py-2">Region</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-zinc-800">
          {loading
            ? Array.from({ length: 4 }).map((_, i) => <LoadRow key={i} />)
            : nodes.length === 0
            ? (
              <tr>
                <td colSpan={6} className="px-3 py-8 text-center text-zinc-500 text-sm">
                  No fleet nodes registered
                </td>
              </tr>
            )
            : nodes.map((node) => (
              <tr key={node.id} className="hover:bg-zinc-800/50 transition-colors">
                <td className="px-3 py-2.5 text-zinc-100 font-mono text-xs">{node.hostname}</td>
                <td className="px-3 py-2.5">
                  <Badge variant={statusVariant(node.status)}>
                    {node.status}
                  </Badge>
                </td>
                <td className="px-3 py-2.5 text-zinc-300 font-mono text-xs">{node.agent_count}</td>
                <td className="px-3 py-2.5">
                  <div className="flex items-center gap-1.5">
                    <div className="w-14 h-1.5 bg-zinc-700 rounded-full overflow-hidden">
                      <div
                        className="h-full bg-blue-500 rounded-full"
                        style={{ width: `${node.cpu_pct}%` }}
                      />
                    </div>
                    <span className="text-zinc-400 text-xs font-mono">{node.cpu_pct}%</span>
                  </div>
                </td>
                <td className="px-3 py-2.5">
                  <div className="flex items-center gap-1.5">
                    <div className="w-14 h-1.5 bg-zinc-700 rounded-full overflow-hidden">
                      <div
                        className="h-full bg-purple-500 rounded-full"
                        style={{ width: `${node.mem_pct}%` }}
                      />
                    </div>
                    <span className="text-zinc-400 text-xs font-mono">{node.mem_pct}%</span>
                  </div>
                </td>
                <td className="px-3 py-2.5 text-zinc-400 text-xs">{node.region}</td>
              </tr>
            ))}
        </tbody>
      </table>
    </div>
  );
}
