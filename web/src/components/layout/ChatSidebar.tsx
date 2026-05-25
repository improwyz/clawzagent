import { useState, useRef, useCallback, useEffect } from 'react';
import { ChatInput } from '../chat/ChatInput';
import { MessageBubble, type Message } from '../chat/MessageBubble';
import { useAppStore } from '../../lib/store';
import { runAgent } from '../../lib/api';
import { connectAgentStream } from '../../lib/ws';

export function ChatSidebar({ width, onResize }: { width: number; onResize: (w: number) => void }) {
  const isResizing = useRef(false);
  const startXRef = useRef(0);
  const startWidthRef = useRef(width);
  const { chatAgentId } = useAppStore();
  const [messages, setMessages] = useState<Message[]>([
    {
      id: '0',
      role: 'assistant',
      content: 'Welcome to ClawZ. How can I help you today?',
      timestamp: new Date().toISOString(),
    },
  ]);
  const [sending, setSending] = useState(false);
  const bottomRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    isResizing.current = true;
    startXRef.current = e.clientX;
    startWidthRef.current = width;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }, [width]);

  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      if (!isResizing.current) return;
      const delta = startXRef.current - e.clientX;
      const newWidth = Math.max(240, Math.min(480, startWidthRef.current + delta));
      onResize(newWidth);
    };
    const handleMouseUp = () => {
      isResizing.current = false;
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
    window.addEventListener('mousemove', handleMouseMove);
    window.addEventListener('mouseup', handleMouseUp);
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', handleMouseUp);
    };
  }, [onResize]);

  const handleSend = async (text: string) => {
    const userMsg: Message = {
      id: Date.now().toString(),
      role: 'user',
      content: text,
      timestamp: new Date().toISOString(),
    };
    setMessages((m) => [...m, userMsg]);
    setSending(true);

    const asstId = (Date.now() + 1).toString();
    const streamMsg: Message = {
      id: asstId,
      role: 'assistant',
      content: '',
      timestamp: new Date().toISOString(),
      streaming: true,
    };
    setMessages((m) => [...m, streamMsg]);

    const agentId = chatAgentId ?? 'default';
    try {
      await runAgent(agentId, text);
      if (wsRef.current) wsRef.current.close();
      wsRef.current = connectAgentStream(agentId, (data) => {
        const d = data as { token?: string; done?: boolean };
        if (d.token) {
          setMessages((m) =>
            m.map((msg) =>
              msg.id === asstId ? { ...msg, content: msg.content + d.token } : msg,
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
    <div
      className="relative flex flex-col bg-zinc-900 border-l border-zinc-800"
      style={{ width: `${width}px` }}
    >
      <div
        className="absolute left-0 top-0 bottom-0 w-1 cursor-col-resize hover:bg-blue-500 transition-colors z-10"
        onMouseDown={handleMouseDown}
      />

      <div className="flex items-center justify-between px-3 py-2.5 border-b border-zinc-800">
        <div className="flex items-center gap-2">
          <div className="w-2 h-2 rounded-full bg-green-400" />
          <h3 className="text-zinc-100 font-medium text-sm">
            {chatAgentId ? `Agent: ${chatAgentId}` : 'ClawZ Chat'}
          </h3>
        </div>
        <button
          onClick={() => setMessages([{
            id: Date.now().toString(),
            role: 'system',
            content: 'Conversation cleared',
            timestamp: new Date().toISOString(),
          }])}
          className="text-zinc-500 hover:text-zinc-300 text-xs transition-colors"
          title="Clear chat"
        >
          ⟳
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-3">
        {messages.map((msg) => (
          <MessageBubble key={msg.id} message={msg} />
        ))}
        <div ref={bottomRef} />
      </div>

      <div className="p-3 border-t border-zinc-800">
        <ChatInput onSend={handleSend} disabled={sending} />
      </div>
    </div>
  );
}
