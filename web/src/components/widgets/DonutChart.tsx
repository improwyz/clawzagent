import { PieChart, Pie, Cell, Tooltip, ResponsiveContainer, Legend } from 'recharts';

const COLORS = ['#3b82f6', '#10b981', '#f59e0b', '#ef4444', '#8b5cf6', '#06b6d4', '#f97316'];

interface DonutChartProps {
  data: { name: string; value: number }[];
  title?: string;
  loading?: boolean;
}

export function DonutChart({ data, title, loading = false }: DonutChartProps) {
  if (loading) {
    return (
      <div className="h-52 flex items-center justify-center">
        <div className="w-32 h-32 rounded-full border-8 border-zinc-800 border-t-blue-500 animate-spin" />
      </div>
    );
  }

  return (
    <div>
      {title && <div className="text-zinc-400 text-xs font-medium uppercase tracking-wider mb-2">{title}</div>}
      <ResponsiveContainer width="100%" height={200}>
        <PieChart>
          <Pie
            data={data}
            cx="50%"
            cy="50%"
            innerRadius={55}
            outerRadius={78}
            paddingAngle={3}
            dataKey="value"
          >
            {data.map((_entry, index) => (
              <Cell key={`cell-${index}`} fill={COLORS[index % COLORS.length]} stroke="transparent" />
            ))}
          </Pie>
          <Tooltip
            contentStyle={{
              background: '#18181b',
              border: '1px solid #3f3f46',
              borderRadius: 8,
              fontSize: 12,
            }}
            itemStyle={{ color: '#e4e4e7' }}
          />
          <Legend
            iconType="circle"
            iconSize={8}
            wrapperStyle={{ fontSize: 11, color: '#71717a' }}
          />
        </PieChart>
      </ResponsiveContainer>
    </div>
  );
}
