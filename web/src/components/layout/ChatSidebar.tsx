import { useRef, useCallback, useEffect } from 'react';
import { ChatPanel } from '../chat/ChatPanel';
import { useAppStore } from '../../lib/store';
import { useQuery } from '@tanstack/react-query';
import { fetchRooms } from '../../lib/api';

export function ChatSidebar({ width, onResize }: { width: number; onResize: (w: number) => void }) {
  const isResizing = useRef(false);
  const startXRef = useRef(0);
  const startWidthRef = useRef(width);
  const { chatAgentId, chatRoomId, setChatRoomId } = useAppStore();

  const { data: rooms = [] } = useQuery({
    queryKey: ['rooms'],
    queryFn: fetchRooms,
    refetchInterval: 30_000,
  });

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

  const activeRoom = rooms.find((r) => r.id === chatRoomId);

  return (
    <div
      className="relative flex flex-col bg-zinc-900 border-l border-zinc-800"
      style={{ width: `${width}px` }}
    >
      <div
        className="absolute left-0 top-0 bottom-0 w-1 cursor-col-resize hover:bg-blue-500 transition-colors z-10"
        onMouseDown={handleMouseDown}
      />

      <div className="flex items-center justify-between px-3 py-2 border-b border-zinc-800 gap-2">
        <div className="flex items-center gap-2 min-w-0">
          <div className="w-2 h-2 rounded-full bg-green-400 flex-shrink-0" />
          <h3 className="text-zinc-100 font-medium text-sm truncate">
            {chatRoomId
              ? (activeRoom?.title ?? `Room ${chatRoomId.slice(0, 8)}`)
              : chatAgentId
                ? `Agent: ${chatAgentId}`
                : 'ClawZ Chat'}
          </h3>
        </div>
        {rooms.length > 0 && (
          <select
            className="text-xs bg-zinc-800 border border-zinc-700 text-zinc-300 rounded px-1.5 py-0.5 max-w-[120px] truncate"
            value={chatRoomId ?? ''}
            onChange={(e) => setChatRoomId(e.target.value || null)}
            title="Select room"
          >
            <option value="">1:1 agent</option>
            {rooms.map((r) => (
              <option key={r.id} value={r.id}>
                {r.title ?? r.id.slice(0, 8)}
              </option>
            ))}
          </select>
        )}
      </div>

      <div className="flex-1 overflow-hidden min-h-0">
        {chatRoomId ? (
          <ChatPanel roomId={chatRoomId} />
        ) : (
          <ChatPanel agentId={chatAgentId ?? 'default'} />
        )}
      </div>
    </div>
  );
}
