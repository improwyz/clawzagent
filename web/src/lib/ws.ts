/** Same-origin ws by default; set VITE_GATEWAY_ORIGIN=http://host:3000 when UI is on another port. */
function wsBaseUrl(): string {
  const env = import.meta.env.VITE_GATEWAY_ORIGIN as string | undefined;
  if (env) {
    const u = new URL(env);
    const proto = u.protocol === 'https:' ? 'wss:' : 'ws:';
    return `${proto}//${u.host}`;
  }
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${location.host}`;
}

function parseWsPayload(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

/** Normalize gateway `{ type, data }` envelopes and legacy flat payloads. */
export function normalizeWsEvent(data: unknown): Record<string, unknown> {
  if (!data || typeof data !== 'object') return {};
  const ev = data as Record<string, unknown>;
  if (
    typeof ev.type === 'string' &&
    ev.data != null &&
    typeof ev.data === 'object' &&
    !Array.isArray(ev.data)
  ) {
    return { type: ev.type, ...(ev.data as Record<string, unknown>) };
  }
  return ev;
}

/** Parse LLM stream frames: legacy `{ token, done }` or gateway `{ type, data }`. */
export function parseAgentStreamEvent(data: unknown): { token?: string; done?: boolean } {
  const ev = normalizeWsEvent(data);
  if (typeof ev.token === 'string') {
    return { token: ev.token, done: ev.done === true };
  }
  if (ev.type === 'token' && typeof ev.data === 'string') {
    return { token: ev.data };
  }
  if (ev.type === 'TurnComplete' && typeof ev.output === 'string') {
    return { token: ev.output };
  }
  if (ev.type === 'done' || ev.type === 'SessionEnd') {
    return { done: true };
  }
  return {};
}

/** Tool timeline events from gateway platform bus (normalized envelope). */
export function parseToolTimelineEvent(
  data: unknown,
): { kind: 'tool_start' | 'tool_end'; tool: string; success?: boolean; preview?: string } | null {
  const ev = normalizeWsEvent(data);
  const t = ev.type as string | undefined;
  if (t === 'agent.turn.tool_start' || t === 'ToolStart') {
    return {
      kind: 'tool_start',
      tool: String(ev.tool_name ?? 'tool'),
    };
  }
  if (t === 'agent.turn.tool_end' || t === 'ToolEnd') {
    return {
      kind: 'tool_end',
      tool: String(ev.tool_name ?? 'tool'),
      success: ev.success !== false,
      preview: typeof ev.output_preview === 'string' ? ev.output_preview : undefined,
    };
  }
  return null;
}

export function connectTurnEvents(onData: (data: unknown) => void): WebSocket {
  const ws = new WebSocket(`${wsBaseUrl()}/ws/events`);
  ws.onmessage = (e) => {
    const parsed = parseWsPayload(e.data as string);
    if (parsed != null) onData(normalizeWsEvent(parsed));
  };
  ws.onerror = () => { /* silently handle */ };
  return ws;
}

export function connectMetrics(onData: (data: unknown) => void): WebSocket {
  const ws = new WebSocket(`${wsBaseUrl()}/ws/metrics`);
  ws.onmessage = (e) => {
    const parsed = parseWsPayload(e.data as string);
    if (parsed != null) onData(normalizeWsEvent(parsed));
  };
  ws.onerror = () => { /* silently handle */ };
  return ws;
}

export function connectEvents(onData: (data: unknown) => void): WebSocket {
  const ws = new WebSocket(`${wsBaseUrl()}/ws/events`);
  ws.onmessage = (e) => {
    const parsed = parseWsPayload(e.data as string);
    if (parsed != null) onData(normalizeWsEvent(parsed));
  };
  ws.onerror = () => { /* silently handle */ };
  return ws;
}

export function connectLogs(onData: (data: unknown) => void): WebSocket {
  const ws = new WebSocket(`${wsBaseUrl()}/ws/logs`);
  ws.onmessage = (e) => {
    const parsed = parseWsPayload(e.data as string);
    if (parsed != null) onData(normalizeWsEvent(parsed));
  };
  ws.onerror = () => { /* silently handle */ };
  return ws;
}

export function connectAgentStream(
  agentId: string,
  onData: (data: unknown) => void,
): WebSocket {
  // Token stream: `/ws/agent/{id}/stream` (alias). Autonomous activity uses `/ws/agents/{id}/stream`.
  const ws = new WebSocket(`${wsBaseUrl()}/ws/agent/${agentId}/stream`);
  ws.onmessage = (e) => {
    const parsed = parseWsPayload(e.data as string);
    if (parsed != null) onData(parseAgentStreamEvent(parsed));
  };
  ws.onerror = () => { /* silently handle */ };
  return ws;
}

export function connectRoomStream(
  roomId: string,
  onEvent: (data: unknown) => void,
): WebSocket {
  const ws = new WebSocket(`${wsBaseUrl()}/ws/rooms/${roomId}`);
  ws.onmessage = (e) => {
    const parsed = parseWsPayload(e.data as string);
    if (parsed != null) onEvent(normalizeWsEvent(parsed));
  };
  ws.onerror = () => { /* silently handle */ };
  return ws;
}
