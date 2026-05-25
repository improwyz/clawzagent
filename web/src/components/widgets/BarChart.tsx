import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
  CartesianGrid,
} from 'recharts';

interface DataPoint {
  time?: string;
  channel?: string;
  label?: string;
  count?: number;
  messages?: number;
  value?: number;
  [key: string]: string | number | undefined;
}

interface MessagesBarChartProps {
  data: DataPoint[];
  dataKey?: string;
  xKey?: string;
  title?: string;
  color?: string;
  loading?: boolean;
}

export function MessagesBarChart({
  data,
  dataKey = 'count',
  xKey = 'time',
  title,
  color = '#3b82f6',
  loading = false,
}: MessagesBarChartProps) {
  if (loading) {
    return (
      <div className="h-52 flex items-end gap-1 px-2 pb-2">
        {Array.from({ length: 12 }).map((_, i) => (
          <div
            key={i}
            className="flex-1 bg-zinc-800 rounded-t animate-pulse"
            style={{ height: `${20 + Math.random() * 70}%` }}
          />
        ))}
      </div>
    );
  }

  return (
    <div>
      {title && <div className="text-zinc-400 text-xs font-medium uppercase tracking-wider mb-2">{title}</div>}
      <ResponsiveContainer width="100%" height={200}>
        <BarChart data={data} margin={{ top: 4, right: 4, bottom: 4, left: -20 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="#27272a" vertical={false} />
          <XAxis
            dataKey={xKey}
            stroke="#52525b"
            tick={{ fill: '#71717a', fontSize: 11 }}
            tickLine={false}
            axisLine={false}
          />
          <YAxis
            stroke="#52525b"
            tick={{ fill: '#71717a', fontSize: 11 }}
            tickLine={false}
            axisLine={false}
          />
          <Tooltip
            contentStyle={{
              background: '#18181b',
              border: '1px solid #3f3f46',
              borderRadius: 8,
              fontSize: 12,
            }}
            labelStyle={{ color: '#a1a1aa' }}
            itemStyle={{ color: color }}
          />
          <Bar dataKey={dataKey} fill={color} radius={[3, 3, 0, 0]} maxBarSize={32} />
        </BarChart>
      </ResponsiveContainer>
    </div>
  );
}
