import { Badge, statusVariant } from '../shared/Badge';
import type { Channel } from '../../lib/api';

interface ChannelGridProps {
  channels: Channel[];
  loading?: boolean;
}

const CHANNEL_ICONS: Record<string, string> = {
  slack: '#',
  discord: 'D',
  telegram: 'T',
  webhook: 'W',
  api: 'A',
  email: '@',
  sms: 'S',
};

export function ChannelGrid({ channels, loading = false }: ChannelGridProps) {
  if (loading) {
    return (
      <div className="grid grid-cols-2 gap-2">
        {Array.from({ length: 6 }).map((_, i) => (
          <div key={i} className="h-16 bg-zinc-800 rounded-lg animate-pulse" />
        ))}
      </div>
    );
  }

  if (channels.length === 0) {
    return (
      <div className="py-6 text-center text-zinc-500 text-sm">No channels configured</div>
    );
  }

  return (
    <div className="grid grid-cols-2 gap-2">
      {channels.map((channel) => {
        const icon = CHANNEL_ICONS[channel.type?.toLowerCase()] ?? '#';
        return (
          <div key={channel.id} className="widget-3d p-3 rounded-lg bg-zinc-900 border border-zinc-800">
            <div className="flex items-center gap-2 mb-1">
              <div className="w-6 h-6 rounded bg-zinc-700 flex items-center justify-center text-xs text-zinc-300 font-bold flex-shrink-0">
                {icon}
              </div>
              <span className="text-zinc-100 text-xs font-medium truncate">{channel.name}</span>
              <Badge variant={statusVariant(channel.status)} className="ml-auto">
                {channel.status}
              </Badge>
            </div>
            <div className="text-zinc-500 text-xs font-mono">{channel.messages.toLocaleString()} msgs</div>
          </div>
        );
      })}
    </div>
  );
}
