function wsBaseUrl(): string {
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
