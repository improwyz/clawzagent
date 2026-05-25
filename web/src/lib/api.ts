const API_BASE = '/api/v1';

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    headers: { 'Content-Type': 'application/json', ...init?.headers },
    ...init,
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`API ${res.status}: ${text}`);
  }
  return res.json() as Promise<T>;
}

// ── Agents ─────────────────────────────────────────────────────────────────
export interface Agent {
  id: string;
  name: string;
  model: string;
  status: 'idle' | 'running' | 'error' | 'stopped';
  last_active: string;
  system_prompt?: string;
  tools?: string[];
  cost_total?: number;
  conversation_count?: number;
}

export function fetchAgents() { return req<Agent[]>('/agents'); }
export function fetchAgent(id: string) { return req<Agent>(`/agents/${id}`); }
export function createAgent(data: Partial<Agent>) {
  return req<Agent>('/agents', { method: 'POST', body: JSON.stringify(data) });
}
export function updateAgent(id: string, data: Partial<Agent>) {
  return req<Agent>(`/agents/${id}`, { method: 'PUT', body: JSON.stringify(data) });
}
export function deleteAgent(id: string) {
  return req<void>(`/agents/${id}`, { method: 'DELETE' });
}
export function runAgent(id: string, message: string) {
  return req<{ run_id: string }>(`/agents/${id}/run`, {
    method: 'POST',
    body: JSON.stringify({ message }),
  });
}

// ── Metrics ────────────────────────────────────────────────────────────────
export interface DashboardMetrics {
  active_agents: number;
  total_conversations: number;
  requests_per_min: number;
  avg_latency_ms: number;
  requests_over_time: { time: string; count: number }[];
  provider_distribution: { name: string; value: number }[];
}

export function fetchMetrics() { return req<DashboardMetrics>('/metrics'); }

// ── Fleet ──────────────────────────────────────────────────────────────────
export interface FleetNode {
  id: string;
  hostname: string;
  status: 'online' | 'offline' | 'busy';
  agent_count: number;
  cpu_pct: number;
  mem_pct: number;
  region: string;
  uptime: string;
  type: string;
}

export interface Deployment {
  id: string;
  agent_id: string;
  agent_name: string;
  node_id: string;
  node_name: string;
  provider: string;
  status: 'running' | 'stopped' | 'failed';
  deployed_at: string;
}

export function fetchFleet() { return req<{ nodes: FleetNode[]; deployments: Deployment[] }>('/fleet'); }
export function deployAgent(agentId: string, nodeId: string, provider: string) {
  return req<Deployment>('/fleet/deploy', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId, node_id: nodeId, provider }),
  });
}

// ── Governance ─────────────────────────────────────────────────────────────
export interface Policy {
  id: string;
  name: string;
  description: string;
  rule: string;
  active: boolean;
  created_at: string;
}

export interface TrustScore {
  agent_id: string;
  agent_name: string;
  score: number;
  tier: 'platinum' | 'gold' | 'silver' | 'bronze' | 'restricted';
}

export interface AuditLog {
  id: string;
  agent_id: string;
  agent_name: string;
  action: string;
  outcome: 'allowed' | 'blocked' | 'flagged';
  timestamp: string;
  details?: string;
}

export interface Approval {
  id: string;
  agent_id: string;
  agent_name: string;
  action: string;
  details: string;
  requested_at: string;
  status: 'pending' | 'approved' | 'rejected';
}

export interface GovernanceData {
  policies: Policy[];
  trust_scores: TrustScore[];
  audit_logs: AuditLog[];
  approvals: Approval[];
  prism_scores: { dimension: string; score: number; status: 'pass' | 'warn' | 'fail' }[];
}

export function fetchGovernance() { return req<GovernanceData>('/governance'); }
export function createPolicy(data: Partial<Policy>) {
  return req<Policy>('/governance/policies', { method: 'POST', body: JSON.stringify(data) });
}
export function updatePolicy(id: string, data: Partial<Policy>) {
  return req<Policy>(`/governance/policies/${id}`, { method: 'PUT', body: JSON.stringify(data) });
}
export function deletePolicy(id: string) {
  return req<void>(`/governance/policies/${id}`, { method: 'DELETE' });
}
export function approveAction(id: string) {
  return req<Approval>(`/governance/approvals/${id}/approve`, { method: 'POST' });
}
export function rejectAction(id: string) {
  return req<Approval>(`/governance/approvals/${id}/reject`, { method: 'POST' });
}

// ── Channels ───────────────────────────────────────────────────────────────
export interface Channel {
  id: string;
  name: string;
  type: string;
  status: 'active' | 'idle' | 'error';
  messages: number;
  enabled: boolean;
}

export function fetchChannels() { return req<Channel[]>('/channels'); }
export function updateChannel(id: string, data: Partial<Channel>) {
  return req<Channel>(`/channels/${id}`, { method: 'PUT', body: JSON.stringify(data) });
}
export function testChannel(id: string) {
  return req<{ success: boolean; message: string }>(`/channels/${id}/test`, { method: 'POST' });
}

// ── Tools ──────────────────────────────────────────────────────────────────
export interface Tool {
  id: string;
  name: string;
  category: string;
  description: string;
  enabled: boolean;
  icon?: string;
}

export interface McpServer {
  id: string;
  name: string;
  url: string;
  status: 'connected' | 'disconnected' | 'error';
  tools_count: number;
}

export interface DockerTool {
  id: string;
  name: string;
  image: string;
  status: 'running' | 'stopped' | 'error';
  port?: number;
}

export interface ToolsData {
  tools: Tool[];
  mcp_servers: McpServer[];
  docker_tools: DockerTool[];
}

export function fetchTools() { return req<ToolsData>('/tools'); }
export function toggleTool(id: string, enabled: boolean) {
  return req<Tool>(`/tools/${id}`, { method: 'PUT', body: JSON.stringify({ enabled }) });
}
export function executeTool(id: string, args: Record<string, unknown>) {
  return req<{ result: unknown; duration_ms: number }>(`/tools/${id}/execute`, {
    method: 'POST',
    body: JSON.stringify({ args }),
  });
}

// ── Config / Providers ─────────────────────────────────────────────────────
export interface Provider {
  id: string;
  name: string;
  type: string;
  api_key_masked?: string;
  enabled: boolean;
  models?: string[];
}

export interface ConfigData {
  providers: Provider[];
  channels: Channel[];
  default_provider: string;
  docker_registry: string;
  cloudflare: Record<string, boolean>;
  saas_connectors: { id: string; name: string; connected: boolean; icon?: string }[];
  system: { port: number; jwt_secret_masked: string };
}

export function fetchConfig() { return req<ConfigData>('/config'); }
export function updateProvider(id: string, data: Partial<Provider>) {
  return req<Provider>(`/config/providers/${id}`, { method: 'PUT', body: JSON.stringify(data) });
}
export function createProvider(data: Partial<Provider>) {
  return req<Provider>('/config/providers', { method: 'POST', body: JSON.stringify(data) });
}
export function rotateJwtSecret() {
  return req<{ secret_masked: string }>('/config/system/rotate-jwt', { method: 'POST' });
}
export function exportConfig() {
  return req<Record<string, unknown>>('/config/export');
}
export function updateCloudflare(service: string, enabled: boolean) {
  return req<void>('/config/cloudflare', {
    method: 'PUT',
    body: JSON.stringify({ service, enabled }),
  });
}
export function connectSaas(id: string) {
  return req<void>(`/config/saas/${id}/connect`, { method: 'POST' });
}
export function disconnectSaas(id: string) {
  return req<void>(`/config/saas/${id}/disconnect`, { method: 'POST' });
}
