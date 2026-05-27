import { clsx } from 'clsx';
import { Badge } from '../shared/Badge';

export type MessageKind =
  | 'user_text'
  | 'agent_text'
  | 'system'
  | 'delegation'
  | 'approval_request';

export type MessageVisibility = 'room' | 'private' | 'internal' | 'side_thread';

export interface Message {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: string;
  streaming?: boolean;
  senderId?: string;
  senderName?: string;
  senderType?: 'user' | 'agent';
  roleBadge?: string;
  messageKind?: MessageKind;
  visibility?: MessageVisibility;
  seq?: number;
}

interface MessageBubbleProps {
  message: Message;
  compact?: boolean;
}

function avatarColor(id?: string): string {
  if (!id) return 'bg-zinc-700';
  const hues = [
    'bg-blue-600',
    'bg-emerald-600',
    'bg-violet-600',
    'bg-amber-600',
    'bg-rose-600',
    'bg-cyan-600',
  ];
  let hash = 0;
  for (let i = 0; i < id.length; i++) hash = (hash + id.charCodeAt(i)) % hues.length;
  return hues[hash] ?? 'bg-zinc-700';
}

function avatarEmoji(senderType?: 'user' | 'agent', role?: string): string {
  if (senderType === 'user') return '👤';
  if (role === 'leader') return '⭐';
  if (role === 'observer') return '👁';
  return '🤖';
}

function roleBadgeVariant(role?: string): 'info' | 'purple' | 'warning' | 'default' {
  switch (role?.toLowerCase()) {
    case 'leader':
      return 'purple';
    case 'owner':
      return 'info';
    case 'observer':
      return 'warning';
    default:
      return 'default';
  }
}

export function MessageBubble({ message, compact }: MessageBubbleProps) {
  const isUser = message.role === 'user' || message.senderType === 'user';
  const isSystem = message.role === 'system' || message.messageKind === 'system';
  const isInternal =
    message.visibility === 'internal' ||
    message.messageKind === 'delegation' ||
    message.messageKind === 'approval_request';

  if (isSystem && !isInternal) {
    return (
      <div className="flex justify-center my-2">
        <span className="text-xs text-zinc-500 bg-zinc-800 px-3 py-1 rounded-full">
          {message.content}
        </span>
      </div>
    );
  }

  const displayName =
    message.senderName ??
    (isUser ? 'You' : message.senderId?.slice(0, 8) ?? 'Agent');

  return (
    <div
      className={clsx(
        'flex gap-2 mb-3',
        isUser && 'flex-row-reverse',
        compact && 'mb-2',
      )}
    >
      <div
        className={clsx(
          'w-7 h-7 rounded-full flex items-center justify-center text-xs flex-shrink-0 mt-0.5',
          isUser ? 'bg-blue-600' : avatarColor(message.senderId),
        )}
        title={displayName}
      >
        {avatarEmoji(message.senderType, message.roleBadge)}
      </div>
      <div className={clsx('max-w-[80%]', isUser && 'items-end flex flex-col')}>
        {!isUser && message.senderName && (
          <div className="flex items-center gap-1.5 mb-0.5 px-1">
            <span className="text-xs text-zinc-400 font-medium">{displayName}</span>
            {message.roleBadge && (
              <Badge variant={roleBadgeVariant(message.roleBadge)} className="text-[10px] py-0">
                {message.roleBadge}
              </Badge>
            )}
            {message.messageKind === 'delegation' && (
              <Badge variant="warning" className="text-[10px] py-0">delegation</Badge>
            )}
            {message.messageKind === 'approval_request' && (
              <Badge variant="error" className="text-[10px] py-0">approval</Badge>
            )}
          </div>
        )}
        <div
          className={clsx(
            'rounded-xl px-3 py-2 text-sm leading-relaxed',
            isUser
              ? 'bg-blue-600 text-white'
              : isInternal
                ? 'bg-zinc-900 text-zinc-400 border border-zinc-700 border-l-2 border-l-amber-600/60'
                : 'bg-zinc-800 text-zinc-200',
          )}
        >
          <p className="whitespace-pre-wrap break-words">{message.content}</p>
          {message.streaming && (
            <span className="inline-block w-1.5 h-3.5 bg-current ml-0.5 animate-pulse" />
          )}
          <div
            className={clsx(
              'text-xs mt-1 opacity-50',
              isUser ? 'text-right' : 'text-left',
            )}
          >
            {new Date(message.timestamp).toLocaleTimeString([], {
              hour: '2-digit',
              minute: '2-digit',
            })}
            {message.seq != null && (
              <span className="ml-1.5 opacity-60">#{message.seq}</span>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

export function roomMessageToBubble(msg: {
  id: string;
  sender_type: string;
  sender_id: string;
  sender_name?: string;
  content: string;
  message_kind?: MessageKind;
  visibility: MessageVisibility;
  timestamp?: string;
  created_at?: string;
  seq?: number;
}, participants?: { participant_id: string; name?: string; role?: string }[]): Message {
  const participant = participants?.find((p) => p.participant_id === msg.sender_id);
  const isUser = msg.sender_type === 'user';
  const kind =
    msg.message_kind ??
    (isUser ? 'user_text' : msg.sender_type === 'agent' ? 'agent_text' : 'system');
  const isSystem = kind === 'system';

  return {
    id: msg.id,
    role: isSystem ? 'system' : isUser ? 'user' : 'assistant',
    content: msg.content,
    timestamp: msg.timestamp ?? msg.created_at ?? new Date().toISOString(),
    senderId: msg.sender_id,
    senderName: msg.sender_name ?? participant?.name,
    senderType: isUser ? 'user' : msg.sender_type === 'agent' ? 'agent' : undefined,
    roleBadge: participant?.role,
    messageKind: kind,
    visibility: msg.visibility,
    seq: msg.seq,
  };
}
