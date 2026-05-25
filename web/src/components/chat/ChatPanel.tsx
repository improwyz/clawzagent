import { useState, useRef, useEffect } from 'react';
import { MessageBubble, type Message } from './MessageBubble';
import { connectAgentStream } from '../../lib/ws';
import { runAgent } from '../../lib/api';

interface ChatPanelProps {
  agentId: string;
  agentName?: string;
}

export function ChatPanel({ agentId, agentName }: ChatPanelProps) {
  const [messages, setMessages] = useState<Message[]>([
    {
      id: '0',
      role: 'system',
      content: `Connected to ${agentName ?? agentId}`,
      timestamp: new Date().toISOString(),
    },
  ]);
  const [input, setInput] = useState('');
  const [sending, setSending] = useState(false);
  const bottomRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const streamMsgIdRef = useRef<string | null>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const send = async () => {
    const text = input.trim();
    if (!text || sending) return;
    setInput('');
    setSending(true);

    const userMsg: Message = {
      id: Date.now().toString(),
      role: 'user',
      content: text,
      timestamp: new Date().toISOString(),
    };
    setMessages((m) => [...m, userMsg]);

    // Start streaming response
    const asstId = (Date.now() + 1).toString();
    streamMsgIdRef.current = asstId;
    const streamMsg: Message = {
      id: asstId,
      role: 'assistant',
      content: '',
      timestamp: new Date().toISOString(),
      streaming: true,
    };
    setMessages((m) => [...m, streamMsg]);

    try {
      await runAgent(agentId, text);
      // Open WebSocket to stream response
      if (wsRef.current) wsRef.current.close();
      wsRef.current = connectAgentStream(agentId, (data) => {
        const d = data as { token?: string; done?: boolean };
        if (d.token) {
          setMessages((m) =>
            m.map((msg) =>
              msg.id === asstId
                ? { ...msg, content: msg.content + d.token }
                : msg,
            ),
          );
        }
        if (d.done) {
          setMessages((m) =>
            m.map((msg) =>
              msg.id === asstId ? { ...msg, streaming: false } : msg,
            ),
          );
          setSending(false);
          wsRef.current?.close();
        }
      });
    } catch (err) {
      setMessages((m) =>
        m.map((msg) =>
          msg.id === asstId
            ? { ...msg, content: `Error: ${String(err)}`, streaming: false }
            : msg,
        ),
      );
      setSending(false);
    }
  };

  return (
    <div className="flex flex-col h-full">
      <div className="flex-1 overflow-y-auto p-3">
        {messages.map((msg) => (
          <MessageBubble key={msg.id} message={msg} />
        ))}
        <div ref={bottomRef} />
      </div>
      <div className="p-3 border-t border-zinc-700">
        <div className="flex gap-2">
          <input
            className="flex-1 bg-zinc-800 text-zinc-100 text-sm px-3 py-2 rounded-lg border border-zinc-700 focus:outline-none focus:border-blue-500 placeholder-zinc-500"
            placeholder={sending ? 'Waiting for response...' : 'Message agent...'}
            value={input}
            disabled={sending}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); } }}
          />
          <button
            onClick={send}
            disabled={sending || !input.trim()}
            className="px-3 py-2 bg-blue-600 text-white rounded-lg text-sm hover:bg-blue-500 disabled:opacity-40 transition-colors"
          >
            ↑
          </button>
        </div>
      </div>
    </div>
  );
}
