export function connectMetrics(onData: (data: unknown) => void): WebSocket {
  const ws = new WebSocket(`ws://${location.host}/ws/metrics`);
  ws.onmessage = (e) => {
    try { onData(JSON.parse(e.data as string)); } catch { /* ignore */ }
  };
  ws.onerror = () => { /* silently handle */ };
  return ws;
}

export function connectEvents(onData: (data: unknown) => void): WebSocket {
  const ws = new WebSocket(`ws://${location.host}/ws/events`);
  ws.onmessage = (e) => {
    try { onData(JSON.parse(e.data as string)); } catch { /* ignore */ }
  };
  ws.onerror = () => { /* silently handle */ };
  return ws;
}

export function connectLogs(onData: (data: unknown) => void): WebSocket {
  const ws = new WebSocket(`ws://${location.host}/ws/logs`);
  ws.onmessage = (e) => {
    try { onData(JSON.parse(e.data as string)); } catch { /* ignore */ }
  };
  ws.onerror = () => { /* silently handle */ };
  return ws;
}

export function connectAgentStream(
  agentId: string,
  onData: (data: unknown) => void,
): WebSocket {
  const ws = new WebSocket(`ws://${location.host}/ws/agent_stream/${agentId}`);
  ws.onmessage = (e) => {
    try { onData(JSON.parse(e.data as string)); } catch { /* ignore */ }
  };
  ws.onerror = () => { /* silently handle */ };
  return ws;
}
