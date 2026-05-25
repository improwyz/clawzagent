import { clsx } from 'clsx';

export interface Message {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: string;
  streaming?: boolean;
}

interface MessageBubbleProps {
  message: Message;
}

export function MessageBubble({ message }: MessageBubbleProps) {
  const isUser = message.role === 'user';
  const isSystem = message.role === 'system';

  if (isSystem) {
    return (
      <div className="flex justify-center my-2">
        <span className="text-xs text-zinc-500 bg-zinc-800 px-3 py-1 rounded-full">
          {message.content}
        </span>
      </div>
    );
  }

  return (
    <div className={clsx('flex gap-2 mb-3', isUser && 'flex-row-reverse')}>
      <div
        className={clsx(
          'w-7 h-7 rounded-full flex items-center justify-center text-xs flex-shrink-0 mt-0.5',
          isUser ? 'bg-blue-600' : 'bg-zinc-700',
        )}
      >
        {isUser ? '👤' : '🤖'}
      </div>
      <div
        className={clsx(
          'max-w-[80%] rounded-xl px-3 py-2 text-sm leading-relaxed',
          isUser
            ? 'bg-blue-600 text-white'
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
        </div>
      </div>
    </div>
  );
}
