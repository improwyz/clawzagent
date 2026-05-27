import { useState, useRef, useEffect, useCallback, useMemo } from 'react';
import { MessageBubble, roomMessageToBubble, type Message } from './MessageBubble';
import { connectAgentStream, connectRoomStream } from '../../lib/ws';
import {
  runAgent,
  getRoom,
  listRoomMessages,
  sendRoomMessage,
  createSideThread,
  orchestrateRoom,
  normalizeRoomMessage,
  type Room,
  type RoomParticipant,
  type RoomMessage,
  type SideThread,
} from '../../lib/api';

interface ChatPanelProps {
  agentId?: string;
  agentName?: string;
  roomId?: string;
  onSideThread?: (threadId: string, roomId: string) => void;
}

function parseMentions(text: string): string[] {
  const matches = text.match(/@([\w.-]+)/g);
  if (!matches) return [];
  return [...new Set(matches.map((m) => m.slice(1)))];
}

function mentionQueryAtCursor(text: string, cursor: number): string | null {
  const before = text.slice(0, cursor);
  const match = before.match(/@([\w.-]*)$/);
  return match ? match[1] : null;
}

export function ChatPanel({
  agentId,
  agentName,
  roomId,
  onSideThread,
}: ChatPanelProps) {
  const isRoomMode = Boolean(roomId);

  const [messages, setMessages] = useState<Message[]>([]);
  const [orgMessages, setOrgMessages] = useState<Message[]>([]);
  const [orgOpen, setOrgOpen] = useState(false);
  const [room, setRoom] = useState<Room | null>(null);
  const [input, setInput] = useState('');
  const [sending, setSending] = useState(false);
  const [loading, setLoading] = useState(isRoomMode);
  const [mentionOpen, setMentionOpen] = useState(false);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [sideThreadLoading, setSideThreadLoading] = useState(false);
  const [activeSideThread, setActiveSideThread] = useState<SideThread | null>(null);

  const bottomRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const streamMsgIdRef = useRef<string | null>(null);
  const lastSeqRef = useRef(0);

  const participants = room?.participants ?? [];

  const mentionCandidates = useMemo(() => {
    const q = mentionQueryAtCursor(input, inputRef.current?.selectionStart ?? input.length);
    if (q === null) return [];
    const lower = q.toLowerCase();
    return participants.filter((p) => {
      const label = p.name ?? p.participant_id;
      return label.toLowerCase().includes(lower);
    });
  }, [input, participants]);

  const applyRoomMessage = useCallback(
    (raw: RoomMessage, streaming = false) => {
      const msg = normalizeRoomMessage(raw);
      const bubble = roomMessageToBubble(msg, participants);
      if (streaming) bubble.streaming = true;

      if (msg.visibility === 'internal' || msg.message_kind === 'delegation') {
        setOrgMessages((prev) => {
          const exists = prev.some((m) => m.id === bubble.id);
          if (exists) {
            return prev.map((m) => (m.id === bubble.id ? { ...m, ...bubble } : m));
          }
          return [...prev, bubble];
        });
        return;
      }

      setMessages((prev) => {
        const exists = prev.some((m) => m.id === bubble.id);
        if (exists) {
          return prev.map((m) => (m.id === bubble.id ? { ...m, ...bubble } : m));
        }
        return [...prev, bubble];
      });
      if (msg.seq > lastSeqRef.current) lastSeqRef.current = msg.seq;
    },
    [participants],
  );

  const handleRoomWsEvent = useCallback(
    (data: unknown) => {
      const ev = data as Record<string, unknown>;
      const type = ev.type as string | undefined;

      if (type === 'connected') return;

      if (type === 'MessageAppend' && ev.message) {
        applyRoomMessage(ev.message as RoomMessage);
        return;
      }

      if (type === 'MessagePatch') {
        const msgId = (ev.message_id ?? ev.id) as string | undefined;
        const patch = ev.patch as Record<string, unknown> | undefined;
        const token =
          (ev.token as string | undefined) ??
          (typeof patch?.content === 'string' ? patch.content : undefined) ??
          (typeof patch?.token === 'string' ? patch.token : undefined);
        const done =
          ev.done === true ||
          patch?.done === true ||
          patch?.streaming === false;
        if (!msgId) return;

        const patchMsg = (m: Message) =>
          m.id === msgId
            ? {
                ...m,
                content: token != null ? m.content + token : m.content,
                streaming: done ? false : true,
              }
            : m;

        setMessages((prev) => prev.map(patchMsg));
        setOrgMessages((prev) => prev.map(patchMsg));
        if (done) setSending(false);
        return;
      }

      if (type === 'SeqGap' && roomId) {
        const after = (ev.after_seq as number) ?? (ev.expected_seq as number) ?? lastSeqRef.current;
        listRoomMessages(roomId, after)
          .then(({ messages: backfill }) => {
            backfill.forEach((m) => applyRoomMessage(m));
          })
          .catch(() => { /* ignore */ });
      }
    },
    [applyRoomMessage, roomId],
  );

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, orgOpen]);

  useEffect(() => {
    if (!roomId) {
      setMessages([
        {
          id: '0',
          role: 'system',
          content: `Connected to ${agentName ?? agentId ?? 'agent'}`,
          timestamp: new Date().toISOString(),
        },
      ]);
      setRoom(null);
      setActiveSideThread(null);
      setLoading(false);
      return;
    }

    let cancelled = false;
    setLoading(true);
    setActiveSideThread(null);
    lastSeqRef.current = 0;

    (async () => {
      try {
        const [roomData, history] = await Promise.all([
          getRoom(roomId),
          listRoomMessages(roomId),
        ]);
        if (cancelled) return;

        setRoom(roomData);
        const main: Message[] = [];
        const org: Message[] = [];

        for (const m of history.messages) {
          if (m.visibility === 'side_thread') continue;
          const bubble = roomMessageToBubble(m, roomData.participants);
          if (m.seq > lastSeqRef.current) lastSeqRef.current = m.seq;
          if (m.visibility === 'internal' || m.message_kind === 'delegation') {
            org.push(bubble);
          } else {
            main.push(bubble);
          }
        }

        if (main.length === 0) {
          main.push({
            id: 'welcome',
            role: 'system',
            content: roomData.title
              ? `Room: ${roomData.title}`
              : `Connected to room (${roomData.room_type})`,
            timestamp: new Date().toISOString(),
          });
        }

        setMessages(main);
        setOrgMessages(org);
        setOrgOpen(org.length > 0);

        if (wsRef.current) wsRef.current.close();
        wsRef.current = connectRoomStream(roomId, handleRoomWsEvent);
      } catch (err) {
        if (!cancelled) {
          setMessages([
            {
              id: 'err',
              role: 'system',
              content: `Failed to load room: ${String(err)}`,
              timestamp: new Date().toISOString(),
            },
          ]);
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
      wsRef.current?.close();
      wsRef.current = null;
    };
  }, [roomId, agentName, agentId, handleRoomWsEvent]);

  const sendAgentDirect = async (text: string) => {
    if (!agentId) return;

    const userMsg: Message = {
      id: Date.now().toString(),
      role: 'user',
      content: text,
      timestamp: new Date().toISOString(),
      senderType: 'user',
    };
    setMessages((m) => [...m, userMsg]);

    const asstId = (Date.now() + 1).toString();
    streamMsgIdRef.current = asstId;
    const streamMsg: Message = {
      id: asstId,
      role: 'assistant',
      content: '',
      timestamp: new Date().toISOString(),
      streaming: true,
      senderType: 'agent',
      senderName: agentName,
    };
    setMessages((m) => [...m, streamMsg]);

    try {
      await runAgent(agentId, text);
      if (wsRef.current) wsRef.current.close();
      wsRef.current = connectAgentStream(agentId, (data) => {
        const d = data as { token?: string; done?: boolean };
        if (d.token) {
          setMessages((m) =>
            m.map((msg) =>
              msg.id === asstId ? { ...msg, content: msg.content + d.token! } : msg,
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

  const sendRoom = async (text: string) => {
    if (!roomId) return;

    const clientId = crypto.randomUUID?.() ?? String(Date.now());
    const mentions = parseMentions(text);

    const inSideThread = Boolean(activeSideThread);

    const optimistic: Message = {
      id: clientId,
      role: 'user',
      content: text,
      timestamp: new Date().toISOString(),
      senderType: 'user',
      messageKind: 'user_text',
      visibility: inSideThread ? 'side_thread' : 'room',
    };
    setMessages((m) => [...m, optimistic]);

    try {
      const result = await sendRoomMessage(roomId, {
        content: text,
        client_message_id: clientId,
        mentions: mentions.length > 0 ? mentions : undefined,
        visibility: inSideThread ? 'side_thread' : 'room',
        thread_id: activeSideThread?.id,
      });
      const saved = result.message;
      setMessages((m) =>
        m.map((msg) =>
          msg.id === clientId ? roomMessageToBubble(saved, participants) : msg,
        ),
      );
      if (saved.seq > lastSeqRef.current) lastSeqRef.current = saved.seq;

      if (result.response) {
        applyRoomMessage(result.response);
      }

      if (room?.room_type === 'agent_team' || room?.orchestration_mode === 'team') {
        await orchestrateRoom(roomId, {
          orchestration_mode: room.orchestration_mode ?? 'team',
        });
      }
    } catch (err) {
      setMessages((m) =>
        m.map((msg) =>
          msg.id === clientId
            ? { ...msg, content: `${text}\n(Error: ${String(err)})` }
            : msg,
        ),
      );
    } finally {
      setSending(false);
    }
  };

  const send = async () => {
    const text = input.trim();
    if (!text || sending) return;
    setInput('');
    setMentionOpen(false);
    setSending(true);

    if (isRoomMode) {
      await sendRoom(text);
    } else {
      await sendAgentDirect(text);
    }
  };

  const insertMention = (p: RoomParticipant) => {
    const label = p.name ?? p.participant_id;
    const cursor = inputRef.current?.selectionStart ?? input.length;
    const before = input.slice(0, cursor).replace(/@([\w.-]*)$/, `@${label} `);
    const after = input.slice(cursor);
    setInput(before + after);
    setMentionOpen(false);
    inputRef.current?.focus();
  };

  const handleInputChange = (value: string) => {
    setInput(value);
    const q = mentionQueryAtCursor(value, inputRef.current?.selectionStart ?? value.length);
    setMentionOpen(q !== null && isRoomMode && participants.length > 0);
    setMentionIndex(0);
  };

  const handleSideThread = async () => {
    if (!roomId || sideThreadLoading) return;
    setSideThreadLoading(true);
    try {
      const thread = await createSideThread(roomId, {
        title: 'Private thread',
      });
      setActiveSideThread(thread);
      setMessages([
        {
          id: `thread-${thread.id}`,
          role: 'system',
          content: thread.title
            ? `Side thread: ${thread.title}`
            : 'Private side thread — messages here stay scoped to this thread.',
          timestamp: new Date().toISOString(),
        },
      ]);
      onSideThread?.(thread.id, roomId);
    } catch (err) {
      setMessages((m) => [
        ...m,
        {
          id: String(Date.now()),
          role: 'system',
          content: `Side thread failed: ${String(err)}`,
          timestamp: new Date().toISOString(),
        },
      ]);
    } finally {
      setSideThreadLoading(false);
    }
  };

  const headerTitle = isRoomMode
    ? activeSideThread
      ? (activeSideThread.title ?? `Side thread ${activeSideThread.id.slice(0, 8)}`)
      : (room?.title ?? `Room ${roomId?.slice(0, 8)}`)
    : (agentName ?? agentId ?? 'Chat');

  return (
    <div className="flex flex-col h-full">
      <div className="px-3 py-2 border-b border-zinc-700 space-y-2">
        <div className="flex items-center justify-between gap-2">
          <h3 className="text-sm font-medium text-zinc-200 truncate">{headerTitle}</h3>
          <div className="flex items-center gap-1 flex-shrink-0">
            {isRoomMode && room?.room_type === 'shared_agent' && !activeSideThread && (
              <button
                type="button"
                onClick={handleSideThread}
                disabled={sideThreadLoading}
                className="px-2 py-0.5 text-xs rounded bg-zinc-800 text-zinc-400 hover:text-zinc-200 hover:bg-zinc-700 disabled:opacity-50"
                title="Private thread with agent"
              >
                {sideThreadLoading ? '…' : 'Side thread'}
              </button>
            )}
            {activeSideThread && (
              <button
                type="button"
                onClick={() => {
                  setActiveSideThread(null);
                  if (roomId) {
                    listRoomMessages(roomId)
                      .then(({ messages: history }) => {
                        const main = history
                          .filter((m) => m.visibility !== 'side_thread')
                          .map((m) => roomMessageToBubble(m, participants));
                        setMessages(main);
                      })
                      .catch(() => { /* ignore */ });
                  }
                }}
                className="px-2 py-0.5 text-xs rounded bg-zinc-800 text-zinc-400 hover:text-zinc-200 hover:bg-zinc-700"
                title="Return to main room"
              >
                Main room
              </button>
            )}
            {isRoomMode && orgMessages.length > 0 && (
              <button
                type="button"
                onClick={() => setOrgOpen((o) => !o)}
                className="px-2 py-0.5 text-xs rounded bg-amber-600/20 text-amber-400 hover:bg-amber-600/30"
              >
                Org {orgMessages.length}
              </button>
            )}
          </div>
        </div>

        {isRoomMode && participants.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {participants.map((p) => (
              <span
                key={`${p.participant_type}-${p.participant_id}`}
                className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-zinc-800 text-xs text-zinc-300"
                title={p.participant_id}
              >
                <span>{p.participant_type === 'user' ? '👤' : '🤖'}</span>
                <span className="truncate max-w-[100px]">
                  {p.name ?? p.participant_id.slice(0, 8)}
                </span>
                {p.role !== 'member' && (
                  <span className="text-[10px] text-zinc-500">{p.role}</span>
                )}
              </span>
            ))}
          </div>
        )}
      </div>

      <div className="flex-1 overflow-y-auto p-3 min-h-0">
        {loading ? (
          <div className="text-center text-zinc-500 text-sm py-8">Loading room…</div>
        ) : (
          messages.map((msg) => <MessageBubble key={msg.id} message={msg} />)
        )}
        <div ref={bottomRef} />
      </div>

      {orgOpen && orgMessages.length > 0 && (
        <div className="border-t border-zinc-700 bg-zinc-950/80 max-h-40 overflow-y-auto">
          <div className="flex items-center justify-between px-3 py-1.5 border-b border-zinc-800">
            <span className="text-xs font-medium text-amber-500/90">Org activity</span>
            <button
              type="button"
              onClick={() => setOrgOpen(false)}
              className="text-zinc-500 hover:text-zinc-300 text-xs"
            >
              Hide
            </button>
          </div>
          <div className="p-2">
            {orgMessages.map((msg) => (
              <MessageBubble key={msg.id} message={msg} compact />
            ))}
          </div>
        </div>
      )}

      <div className="p-3 border-t border-zinc-700 relative">
        {mentionOpen && mentionCandidates.length > 0 && (
          <div className="absolute bottom-full left-3 right-3 mb-1 bg-zinc-800 border border-zinc-600 rounded-lg shadow-lg overflow-hidden z-10 max-h-36 overflow-y-auto">
            {mentionCandidates.map((p, i) => (
              <button
                key={`${p.participant_type}-${p.participant_id}`}
                type="button"
                className={`w-full text-left px-3 py-1.5 text-sm flex items-center gap-2 ${
                  i === mentionIndex
                    ? 'bg-blue-600/30 text-zinc-100'
                    : 'text-zinc-300 hover:bg-zinc-700'
                }`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  insertMention(p);
                }}
              >
                <span>{p.participant_type === 'user' ? '👤' : '🤖'}</span>
                <span>{p.name ?? p.participant_id}</span>
                <span className="text-xs text-zinc-500 ml-auto">{p.role}</span>
              </button>
            ))}
          </div>
        )}
        <div className="flex gap-2">
          <input
            ref={inputRef}
            className="flex-1 bg-zinc-800 text-zinc-100 text-sm px-3 py-2 rounded-lg border border-zinc-700 focus:outline-none focus:border-blue-500 placeholder-zinc-500"
            placeholder={
              sending
                ? 'Waiting for response...'
                : isRoomMode
                  ? 'Message room… (@ to mention)'
                  : 'Message agent...'
            }
            value={input}
            disabled={sending || loading}
            onChange={(e) => handleInputChange(e.target.value)}
            onKeyDown={(e) => {
              if (mentionOpen && mentionCandidates.length > 0) {
                if (e.key === 'ArrowDown') {
                  e.preventDefault();
                  setMentionIndex((i) => Math.min(i + 1, mentionCandidates.length - 1));
                  return;
                }
                if (e.key === 'ArrowUp') {
                  e.preventDefault();
                  setMentionIndex((i) => Math.max(i - 1, 0));
                  return;
                }
                if (e.key === 'Tab' || (e.key === 'Enter' && mentionCandidates[mentionIndex])) {
                  e.preventDefault();
                  insertMention(mentionCandidates[mentionIndex]);
                  return;
                }
                if (e.key === 'Escape') {
                  setMentionOpen(false);
                  return;
                }
              }
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
          />
          <button
            type="button"
            onClick={send}
            disabled={sending || loading || !input.trim()}
            className="px-3 py-2 bg-blue-600 text-white rounded-lg text-sm hover:bg-blue-500 disabled:opacity-40 transition-colors"
          >
            ↑
          </button>
        </div>
      </div>
    </div>
  );
}
