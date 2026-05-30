import { getShellConfig, isTauriShell, loadGatewayApiKey } from './shell';

/** Default: same-origin `/api/v1` (Vite dev/preview proxy). Override at build: VITE_API_BASE=http://host:3000/api/v1 */
const DEFAULT_API_BASE =
  (import.meta.env.VITE_API_BASE as string | undefined)?.replace(/\/$/, '') || '/api/v1';

let runtimeApiBase: string | null = null;

export function normalizeGatewayToApiBase(origin: string): string {
  const u = origin.replace(/\/$/, '');
  return u.endsWith('/api/v1') ? u : `${u}/api/v1`;
}

/** Resolved API base (Tauri shell config overrides build-time default). */
export function getApiBase(): string {
  return runtimeApiBase ?? DEFAULT_API_BASE;
}

/** Load gateway URL + keyring API key when running inside the Tauri desktop shell. */
export async function initShellApi(): Promise<void> {
  if (!isTauriShell()) return;
  const cfg = await getShellConfig();
  if (cfg.gateway_url) {
    runtimeApiBase = normalizeGatewayToApiBase(cfg.gateway_url);
  }
  const key = await loadGatewayApiKey();
  if (key) setStoredApiKey(key);
}

/** Gateway origin without `/api/v1` (for `/health` and other root routes). */
function gatewayOrigin(): string {
  const base = getApiBase().replace(/\/$/, '');
  if (base.endsWith('/api/v1')) {
    return base.slice(0, -'/api/v1'.length) || '';
  }
  return base;
}

export function getStoredAuthToken(): string | null {
  return typeof localStorage !== 'undefined' ? localStorage.getItem('clawz_token') : null;
}

export function getStoredApiKey(): string | null {
  return typeof localStorage !== 'undefined' ? localStorage.getItem('clawz_api_key') : null;
}

export function setStoredApiKey(key: string) {
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('clawz_api_key', key);
  }
}

export function hasAuthCredentials(): boolean {
  return Boolean(
    getStoredAuthToken() ||
      getStoredApiKey() ||
      (import.meta.env.VITE_API_KEY as string | undefined),
  );
}

function authHeaders(): Record<string, string> {
  const token = getStoredAuthToken();
  if (token) return { Authorization: `Bearer ${token}` };
  const apiKey = getStoredApiKey() || (import.meta.env.VITE_API_KEY as string | undefined);
  if (apiKey) return { 'X-API-Key': apiKey };
  return {};
}

let onAuthFailure: (() => void) | null = null;

export function setAuthFailureHandler(handler: (() => void) | null) {
  onAuthFailure = handler;
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${getApiBase()}${path}`, {
    headers: { 'Content-Type': 'application/json', ...authHeaders(), ...init?.headers },
    ...init,
  });
  const text = await res.text();
  if (!res.ok) {
    if ((res.status === 401 || res.status === 403) && onAuthFailure) {
      onAuthFailure();
    }
    throw new Error(`API ${res.status}: ${text.slice(0, 200)}`);
  }
  const ctype = res.headers.get('content-type') ?? '';
  if (!ctype.includes('json') && text.trimStart().startsWith('<')) {
    throw new Error(
      'API returned HTML instead of JSON — is the gateway reachable? ' +
        'For vite preview, set preview.proxy in vite.config.ts or build with VITE_API_BASE=http://host:3000/api/v1',
    );
  }
  if (!text) {
    return undefined as T;
  }
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new Error(`API returned invalid JSON: ${text.slice(0, 120)}`);
  }
}

interface PaginatedResponse<T> {
  data?: T;
  total?: number;
}

function unwrapData<T>(payload: PaginatedResponse<T> | T): T {
  if (payload != null && typeof payload === 'object' && 'data' in payload) {
    return (payload as PaginatedResponse<T>).data as T;
  }
  return payload as T;
}

/** Normalize gateway list payloads: `{ data }`, `{ rooms }`, or a bare array. */
function asArray<T>(payload: unknown): T[] {
  if (Array.isArray(payload)) return payload as T[];
  if (payload != null && typeof payload === 'object') {
    const obj = payload as Record<string, unknown>;
    if (Array.isArray(obj.data)) return obj.data as T[];
    if (Array.isArray(obj.rooms)) return obj.rooms as T[];
    if (Array.isArray(obj.items)) return obj.items as T[];
  }
  return [];
}

function asRecord(value: unknown): Record<string, unknown> {
  return value != null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function normalizeAgent(raw: Record<string, unknown>): Agent {
  const status = String(raw.status ?? 'idle').toLowerCase();
  return {
    id: String(raw.id ?? ''),
    name: String(raw.name ?? 'agent'),
    model: String(raw.model ?? 'stub'),
    status: (['idle', 'running', 'error', 'stopped'].includes(status)
      ? status
      : 'idle') as Agent['status'],
    last_active: String(raw.last_active ?? raw.updated_at ?? new Date().toISOString()),
    system_prompt: raw.system_prompt != null ? String(raw.system_prompt) : undefined,
    tools: Array.isArray(raw.tools) ? (raw.tools as string[]) : undefined,
  };
}

function normalizeDeployment(raw: Record<string, unknown>): Deployment {
  const status = String(raw.status ?? 'stopped').toLowerCase();
  const mapped =
    status === 'complete' || status === 'pending'
      ? status === 'pending'
        ? 'running'
        : 'stopped'
      : status;
  return {
    id: String(raw.id ?? ''),
    agent_id: String(raw.agent_id ?? ''),
    agent_name: String(raw.agent_name ?? ''),
    node_id: String(raw.node_id ?? ''),
    node_name: String(raw.node_name ?? ''),
    provider: String(raw.provider ?? 'fleet'),
    status: (['running', 'stopped', 'failed'].includes(mapped)
      ? mapped
      : 'stopped') as Deployment['status'],
    deployed_at: String(raw.deployed_at ?? raw.created_at ?? new Date().toISOString()),
  };
}

function normalizeFleetNode(raw: Record<string, unknown>): FleetNode {
  const status = String(raw.status ?? 'offline').toLowerCase();
  const agentIds = Array.isArray(raw.agent_ids) ? raw.agent_ids : [];
  const host = String(raw.hostname ?? raw.host ?? raw.name ?? 'localhost');
  return {
    id: String(raw.id ?? ''),
    hostname: host,
    status: (['online', 'offline', 'busy'].includes(status) ? status : 'offline') as FleetNode['status'],
    agent_count: Number(raw.agent_count ?? agentIds.length ?? 0),
    cpu_pct: Number(raw.cpu_pct ?? 0),
    mem_pct: Number(raw.mem_pct ?? 0),
    region: String(raw.region ?? 'default'),
    uptime: String(raw.uptime ?? '—'),
    type: String(raw.type ?? raw.node_type ?? 'worker'),
  };
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

export async function fetchAgents() {
  const res = await req<unknown>('/agents');
  return asArray<Record<string, unknown>>(res).map(normalizeAgent);
}
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
  return req<{ run_id: string; conversation_id?: string; content?: string }>(
    `/agents/${id}/run`,
    {
      method: 'POST',
      body: JSON.stringify({ message }),
    },
  );
}

export function stopAgent(id: string) {
  return req<{ id: string; status: string }>(`/agents/${id}/stop`, { method: 'POST' });
}

export function startAgent(id: string) {
  return req<{ id: string; status: string }>(`/agents/${id}/start`, { method: 'POST' });
}

export function fetchAgentStatus(id: string) {
  return req<{ id: string; status: string }>(`/agents/${id}/status`);
}

export interface AgentHistoryMessage {
  id?: string;
  role: string;
  content: string;
  created_at?: string;
}

export interface AgentHistory {
  agent_id: string;
  messages: AgentHistoryMessage[];
  total: number;
}

export async function fetchAgentHistory(id: string): Promise<AgentHistory> {
  const res = await req<Record<string, unknown>>(`/agents/${id}/history`);
  const messages = asArray<Record<string, unknown>>(res.messages).map((m) => ({
    id: m.id != null ? String(m.id) : undefined,
    role: String(m.role ?? 'user'),
    content: String(m.content ?? ''),
    created_at: m.created_at != null ? String(m.created_at) : undefined,
  }));
  return {
    agent_id: String(res.agent_id ?? id),
    messages,
    total: Number(res.total ?? messages.length),
  };
}

export function runAutonomous(
  id: string,
  opts?: { maxTurns?: number; costBudgetUsd?: number; systemPrompt?: string },
) {
  return req<{ session_id: string; status: string }>(`/agents/${id}/autonomous`, {
    method: 'POST',
    body: JSON.stringify({
      maxTurns: opts?.maxTurns,
      costBudgetUsd: opts?.costBudgetUsd,
      systemPrompt: opts?.systemPrompt,
    }),
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

export function fetchMetrics() {
  return req<DashboardMetrics>('/dashboard/metrics');
}

// ── Dashboard overview ─────────────────────────────────────────────────────

export interface DashboardOverviewHealth {
  status: string;
  uptime_secs: number;
  version: string;
  auth_disabled: boolean;
}

export interface DashboardOverviewCounts {
  agents: number;
  agents_running: number;
  conversations: number;
  rooms: number;
  channels: number;
  providers: number;
  tools: number;
  policies: number;
  fleet_nodes: number;
  fleet_nodes_online: number;
  deployments: number;
  audit_entries: number;
  pending_approvals: number;
}

export interface DashboardApiCatalogItem {
  method: string;
  path: string;
  summary: string;
}

export interface DashboardApiCatalogGroup {
  group: string;
  items: DashboardApiCatalogItem[];
}

export interface DashboardOverview {
  health: DashboardOverviewHealth;
  counts: DashboardOverviewCounts;
  links: Record<string, string>;
  api_catalog: DashboardApiCatalogGroup[];
  generated_at: string;
}

export async function fetchDashboardOverview(): Promise<DashboardOverview> {
  const raw = await req<Record<string, unknown>>('/dashboard/overview');
  const health = asRecord(raw.health);
  const counts = asRecord(raw.counts);
  const catalog = asArray<Record<string, unknown>>(raw.api_catalog).map((g) => ({
    group: String(g.group ?? ''),
    items: asArray<Record<string, unknown>>(g.items).map((item) => ({
      method: String(item.method ?? ''),
      path: String(item.path ?? ''),
      summary: String(item.summary ?? ''),
    })),
  }));
  return {
    health: {
      status: String(health.status ?? 'unknown'),
      uptime_secs: Number(health.uptime_secs ?? 0),
      version: String(health.version ?? ''),
      auth_disabled: Boolean(health.auth_disabled),
    },
    counts: {
      agents: Number(counts.agents ?? 0),
      agents_running: Number(counts.agents_running ?? 0),
      conversations: Number(counts.conversations ?? 0),
      rooms: Number(counts.rooms ?? 0),
      channels: Number(counts.channels ?? 0),
      providers: Number(counts.providers ?? 0),
      tools: Number(counts.tools ?? 0),
      policies: Number(counts.policies ?? 0),
      fleet_nodes: Number(counts.fleet_nodes ?? 0),
      fleet_nodes_online: Number(counts.fleet_nodes_online ?? 0),
      deployments: Number(counts.deployments ?? 0),
      audit_entries: Number(counts.audit_entries ?? 0),
      pending_approvals: Number(counts.pending_approvals ?? 0),
    },
    links: Object.fromEntries(
      Object.entries(asRecord(raw.links)).map(([k, v]) => [k, String(v)]),
    ),
    api_catalog: catalog,
    generated_at: String(raw.generated_at ?? new Date().toISOString()),
  };
}

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

export async function fetchFleet() {
  const [fleetRes, deploymentsRes] = await Promise.all([
    req<unknown>('/fleet'),
    req<unknown>('/fleet/deployments'),
  ]);
  const nodes = asArray<Record<string, unknown>>(fleetRes).map(normalizeFleetNode);
  const deployments = asArray<Record<string, unknown>>(deploymentsRes).map(normalizeDeployment);
  return { nodes, deployments };
}

export function deployAgent(agentId: string, nodeId: string, _provider?: string) {
  return req<Deployment>('/fleet/deploy', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId, node_id: nodeId }),
  }).then((d) => normalizeDeployment(asRecord(d)));
}

export interface FleetMeshNode {
  id: string;
  name: string;
  node_type: string;
  host: string;
  port: number;
  status: string;
  agent_count: number;
}

export interface FleetMeshConnection {
  from: string;
  to: string;
  latency_ms: number;
  bandwidth_mbps?: number;
}

export interface FleetMesh {
  nodes: FleetMeshNode[];
  connections: FleetMeshConnection[];
  total_nodes: number;
  online_nodes: number;
}

export async function fetchFleetMesh(): Promise<FleetMesh> {
  const raw = await req<Record<string, unknown>>('/fleet/mesh');
  return {
    nodes: asArray<Record<string, unknown>>(raw.nodes).map((n) => ({
      id: String(n.id ?? ''),
      name: String(n.name ?? ''),
      node_type: String(n.node_type ?? 'worker'),
      host: String(n.host ?? ''),
      port: Number(n.port ?? 0),
      status: String(n.status ?? 'offline'),
      agent_count: Number(n.agent_count ?? 0),
    })),
    connections: asArray<Record<string, unknown>>(raw.connections).map((c) => ({
      from: String(c.from ?? ''),
      to: String(c.to ?? ''),
      latency_ms: Number(c.latency_ms ?? 0),
      bandwidth_mbps: c.bandwidth_mbps != null ? Number(c.bandwidth_mbps) : undefined,
    })),
    total_nodes: Number(raw.total_nodes ?? 0),
    online_nodes: Number(raw.online_nodes ?? 0),
  };
}

export interface FleetMetrics {
  nodes: { total: number; online: number; offline: number; degraded: number };
  deployments: { total: number; active: number };
  agents: { total: number; running: number };
  computed_at?: string;
}

export function fetchFleetMetrics() {
  return req<FleetMetrics>('/fleet/metrics');
}

export interface KanbanColumn {
  id: string;
  name: string;
  tasks: Record<string, unknown>[];
}

export async function fetchFleetKanban() {
  const raw = await req<Record<string, unknown>>('/fleet/kanban');
  return asArray<Record<string, unknown>>(raw.columns).map((col) => ({
    id: String(col.id ?? ''),
    name: String(col.name ?? ''),
    tasks: asArray<Record<string, unknown>>(col.tasks),
  })) as KanbanColumn[];
}

export interface FleetNodeLog {
  level: string;
  message: string;
  timestamp: string;
}

export async function fetchFleetNodeLogs(nodeId: string) {
  const raw = await req<Record<string, unknown>>(`/fleet/${nodeId}/logs`);
  return {
    node_id: String(raw.node_id ?? nodeId),
    logs: asArray<Record<string, unknown>>(raw.logs).map((l) => ({
      level: String(l.level ?? 'info'),
      message: String(l.message ?? ''),
      timestamp: String(l.timestamp ?? ''),
    })) as FleetNodeLog[],
    total: Number(raw.total ?? 0),
  };
}

export function createFleetNode(data: {
  name: string;
  node_type?: string;
  host?: string;
  port?: number;
}) {
  return req<Record<string, unknown>>('/fleet', {
    method: 'POST',
    body: JSON.stringify(data),
  });
}

export function deleteFleetNode(id: string) {
  return req<void>(`/fleet/${id}`, { method: 'DELETE' });
}

// ── Cloud deploy ───────────────────────────────────────────────────────────
export interface CloudProvider {
  id: string;
  display_name: string;
  deploy_modes: string[];
}

export async function fetchCloudProviders() {
  const res = await req<unknown>('/cloud/providers');
  return asArray<Record<string, unknown>>(res).map(
    (p): CloudProvider => ({
      id: String(p.id ?? ''),
      display_name: String(p.display_name ?? p.name ?? p.id ?? ''),
      deploy_modes: Array.isArray(p.deploy_modes)
        ? (p.deploy_modes as string[])
        : Array.isArray(p.modes)
          ? (p.modes as string[])
          : [],
    }),
  );
}

export interface CloudDeployment {
  id: string;
  provider_id?: string;
  external_resource?: string;
  url?: string;
  status?: string;
  region?: string;
  created_at?: string;
}

export async function fetchCloudDeployments() {
  const res = await req<unknown>('/cloud/deployments');
  return asArray<Record<string, unknown>>(res).map(
    (d): CloudDeployment => ({
      id: String(d.id ?? ''),
      provider_id: d.provider_id != null ? String(d.provider_id) : undefined,
      external_resource:
        d.external_resource != null ? String(d.external_resource) : undefined,
      url: d.url != null ? String(d.url) : undefined,
      status: d.status != null ? String(d.status) : undefined,
      region: d.region != null ? String(d.region) : undefined,
      created_at: d.created_at != null ? String(d.created_at) : undefined,
    }),
  );
}

export function deployToCloud(body: {
  provider_id?: string;
  mode?: string;
  image?: string;
  env_vars?: Record<string, string>;
  region?: string;
  replicas?: number;
}) {
  return req<{ provider_id: string; deployment: CloudDeployment }>('/cloud/deploy', {
    method: 'POST',
    body: JSON.stringify(body),
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

async function safeReq(path: string): Promise<unknown> {
  try {
    return await req<unknown>(path);
  } catch {
    return null;
  }
}

export async function fetchGovernance(): Promise<GovernanceData> {
  const [policiesRes, auditRes, proposalsRes, prismRes, agents] = await Promise.all([
    safeReq('/governance/policies'),
    safeReq('/governance/audit?limit=50'),
    safeReq('/governance/proposals'),
    safeReq('/system/prism'),
    fetchAgents().catch(() => [] as Agent[]),
  ]);

  const policies: Policy[] = asArray<Record<string, unknown>>(policiesRes).map((p) => ({
    id: String(p.id ?? ''),
    name: String(p.name ?? ''),
    description: String(p.description ?? ''),
    rule: Array.isArray(p.rules) ? (p.rules as string[]).join('; ') : String(p.rule ?? ''),
    active: Boolean(p.enabled ?? p.active ?? true),
    created_at: String(p.created_at ?? ''),
  }));

  const audit_logs: AuditLog[] = asArray<Record<string, unknown>>(auditRes).map((e) => ({
    id: String(e.id ?? ''),
    agent_id: String(e.resource_id ?? e.agent_id ?? ''),
    agent_name: String(e.actor ?? e.resource_type ?? 'system'),
    action: String(e.action ?? ''),
    outcome: 'allowed' as const,
    timestamp: String(e.created_at ?? e.timestamp ?? new Date().toISOString()),
    details: e.details != null ? String(e.details) : undefined,
  }));

  const approvals: Approval[] = asArray<Record<string, unknown>>(proposalsRes).map((p) => {
    const agent = agents.find((a) => a.id === p.agent_id);
    return {
      id: String(p.id ?? ''),
      agent_id: String(p.agent_id ?? ''),
      agent_name: agent?.name ?? String(p.agent_id ?? 'agent'),
      action: String(p.action ?? ''),
      details: JSON.stringify(p.context ?? {}),
      requested_at: String(p.created_at ?? new Date().toISOString()),
      status: (String(p.status ?? 'pending').toLowerCase() === 'approved'
        ? 'approved'
        : String(p.status ?? '').toLowerCase() === 'rejected'
          ? 'rejected'
          : 'pending') as Approval['status'],
    };
  });

  const prismRaw = asRecord(prismRes).dimensions;
  const prism_scores = asArray<Record<string, unknown>>(prismRaw).map((d) => {
    const status = String(d.status ?? 'planned');
    return {
      dimension: String(d.dimension ?? d.title ?? ''),
      score: status === 'implemented' ? 900 : status === 'partial' ? 500 : 200,
      status: (status === 'implemented' ? 'pass' : status === 'partial' ? 'warn' : 'fail') as
        | 'pass'
        | 'warn'
        | 'fail',
    };
  });

  const trust_scores: TrustScore[] = agents.map((a) => ({
    agent_id: a.id,
    agent_name: a.name,
    score: a.status === 'running' ? 750 : a.status === 'error' ? 300 : 600,
    tier: (a.status === 'error' ? 'bronze' : 'silver') as TrustScore['tier'],
  }));

  return { policies, trust_scores, audit_logs, approvals, prism_scores };
}
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
  return req<Approval>(`/governance/proposals/${id}/vote`, {
    method: 'POST',
    body: JSON.stringify({ decision: 'approve', approver_id: 'dashboard' }),
  });
}
export function rejectAction(id: string) {
  return req<Approval>(`/governance/proposals/${id}/vote`, {
    method: 'POST',
    body: JSON.stringify({ decision: 'reject', approver_id: 'dashboard', reason: 'rejected' }),
  });
}

export async function fetchPolicies() {
  const res = await req<unknown>('/governance/policies');
  return asArray<Record<string, unknown>>(res).map(
    (p): Policy => ({
      id: String(p.id ?? ''),
      name: String(p.name ?? ''),
      description: String(p.description ?? ''),
      rule: Array.isArray(p.rules) ? (p.rules as string[]).join('; ') : String(p.rule ?? ''),
      active: Boolean(p.enabled ?? p.active ?? true),
      created_at: String(p.created_at ?? ''),
    }),
  );
}

export function fetchPolicy(id: string) {
  return req<Policy>(`/governance/policies/${id}`);
}

export async function fetchAuditLog(params?: {
  page?: number;
  limit?: number;
  resource_type?: string;
  actor?: string;
}) {
  const qs = new URLSearchParams();
  if (params?.page != null) qs.set('page', String(params.page));
  if (params?.limit != null) qs.set('limit', String(params.limit));
  if (params?.resource_type) qs.set('resource_type', params.resource_type);
  if (params?.actor) qs.set('actor', params.actor);
  const q = qs.toString();
  const res = await req<unknown>(`/governance/audit${q ? `?${q}` : ''}`);
  return asArray<Record<string, unknown>>(res).map(
    (e): AuditLog => ({
      id: String(e.id ?? ''),
      agent_id: String(e.resource_id ?? e.agent_id ?? ''),
      agent_name: String(e.actor ?? e.resource_type ?? 'system'),
      action: String(e.action ?? ''),
      outcome: 'allowed',
      timestamp: String(e.created_at ?? e.timestamp ?? new Date().toISOString()),
      details: e.details != null ? String(e.details) : undefined,
    }),
  );
}

export async function fetchProposals() {
  const res = await req<unknown>('/governance/proposals');
  return asArray<Record<string, unknown>>(res);
}

export interface GovernanceTrustResponse {
  agent_id: string;
  trust_score: number;
  total_actions: number;
  violations: number;
  tier: string;
  computed_at?: string;
}

export function fetchTrustScore(agentId: string) {
  return req<GovernanceTrustResponse>(`/governance/trust/${agentId}`);
}

export interface GovernanceEvaluateResult {
  allowed: boolean;
  outcome: string;
  matched_policies?: string[];
  reason?: string;
}

export function evaluateGovernance(agentId: string, action: string, context?: Record<string, unknown>) {
  return req<GovernanceEvaluateResult>('/governance/evaluate', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId, action, context }),
  });
}

export function fetchPrismStatus() {
  return req<Record<string, unknown>>('/system/prism');
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

export async function fetchChannels() {
  const res = await req<PaginatedResponse<Channel[]> | Channel[] | unknown>('/channels');
  const raw = asArray<Channel>(res);
  return raw.map((ch) => ({
    ...ch,
    type: ch.type ?? (ch as { channel_type?: string }).channel_type ?? 'unknown',
    status: ch.status ?? (ch.enabled ? 'active' : 'idle'),
    messages: ch.messages ?? 0,
  }));
}
export function createChannel(data: {
  name: string;
  channel_type: string;
  enabled?: boolean;
  config?: Record<string, unknown>;
}) {
  return req<Channel>('/channels', { method: 'POST', body: JSON.stringify(data) });
}

export function updateChannel(id: string, data: Partial<Channel> & { enabled?: boolean }) {
  return req<Channel>(`/channels/${id}`, {
    method: 'PUT',
    body: JSON.stringify({
      name: data.name,
      channel_type: data.type,
      enabled: data.enabled,
    }),
  });
}
export function testChannel(id: string) {
  return req<{ success: boolean; message: string; id?: string }>(`/channels/${id}/test`, {
    method: 'POST',
  });
}

export function fetchChannel(id: string) {
  return req<Channel>(`/channels/${id}`);
}

export function deleteChannel(id: string) {
  return req<void>(`/channels/${id}`, { method: 'DELETE' });
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
  catalog?: Tool[];
  mcp_servers: McpServer[];
  docker_tools: DockerTool[];
}

function normalizeTool(raw: Record<string, unknown>): Tool {
  return {
    id: String(raw.id ?? ''),
    name: String(raw.name ?? ''),
    category: String(raw.category ?? raw.tool_type ?? 'function'),
    description: String(raw.description ?? ''),
    enabled: Boolean(raw.enabled ?? true),
    icon: raw.icon != null ? String(raw.icon) : undefined,
  };
}

function normalizeDockerTool(raw: Record<string, unknown>): DockerTool {
  const status = String(raw.status ?? 'stopped').toLowerCase();
  return {
    id: String(raw.id ?? ''),
    name: String(raw.name ?? ''),
    image: String(raw.image ?? ''),
    status: (['running', 'stopped', 'error'].includes(status)
      ? status
      : 'stopped') as DockerTool['status'],
    port: raw.port != null ? Number(raw.port) : undefined,
  };
}

function normalizeMcpServer(raw: Record<string, unknown>): McpServer {
  const status = String(raw.status ?? 'disconnected').toLowerCase();
  return {
    id: String(raw.id ?? ''),
    name: String(raw.name ?? ''),
    url: String(raw.url ?? ''),
    status: (['connected', 'disconnected', 'error'].includes(status)
      ? status
      : 'disconnected') as McpServer['status'],
    tools_count: Number(raw.tools_count ?? 0),
  };
}

export async function fetchTools(): Promise<ToolsData> {
  const payload = await req<Record<string, unknown>>('/dashboard/tools');
  const data = asRecord(payload);
  const tools = asArray<Record<string, unknown>>(data.tools).map(normalizeTool);
  const catalog = asArray<Record<string, unknown>>(data.catalog).map(normalizeTool);
  return {
    tools,
    catalog: catalog.length > 0 ? catalog : tools,
    mcp_servers: asArray<Record<string, unknown>>(data.mcp_servers).map(normalizeMcpServer),
    docker_tools: asArray<Record<string, unknown>>(data.docker_tools).map(normalizeDockerTool),
  };
}

export function createTool(data: {
  name: string;
  description?: string;
  tool_type: string;
  config?: Record<string, unknown>;
  enabled?: boolean;
}) {
  return req<Tool>('/tools', { method: 'POST', body: JSON.stringify(data) });
}

export function registerCatalogTool(name: string, toolType: string, description: string) {
  return createTool({
    name,
    description,
    tool_type: toolType,
    enabled: true,
    config: { source: 'catalog' },
  });
}

export function toggleTool(
  id: string,
  enabled: boolean,
  meta?: { category?: string; description?: string },
) {
  if (id.startsWith('catalog-')) {
    if (!enabled) {
      return Promise.reject(new Error('Install the tool first by enabling it'));
    }
    const name = id.slice('catalog-'.length);
    return registerCatalogTool(
      name,
      meta?.category ?? 'function',
      meta?.description ?? `Built-in tool: ${name}`,
    );
  }
  return req<Tool>(`/tools/${id}`, { method: 'PUT', body: JSON.stringify({ enabled }) });
}

export function createMcpServer(data: { name: string; url: string }) {
  return createTool({
    name: data.name,
    description: `MCP server at ${data.url}`,
    tool_type: 'mcp',
    enabled: true,
    config: { url: data.url },
  });
}

export async function executeTool(id: string, args: Record<string, unknown>) {
  const data = await req<{
    result?: { output?: unknown; execution_time_ms?: number; status?: string };
    duration_ms?: number;
  }>(`/tools/${id}/execute`, {
    method: 'POST',
    body: JSON.stringify({ args }),
  });
  const nested = data.result;
  return {
    result: nested?.output ?? nested ?? data,
    duration_ms: nested?.execution_time_ms ?? data.duration_ms ?? 0,
  };
}

// ── Auth ─────────────────────────────────────────────────────────────────────
export interface AuthResponse {
  token: string;
  user_id: string;
  email: string;
  role: string;
}

export interface AuthStatus {
  auth_disabled: boolean;
}

export async function fetchAuthStatus(): Promise<AuthStatus> {
  const res = await fetch(`${getApiBase()}/system/auth/status`);
  if (!res.ok) {
    return { auth_disabled: false };
  }
  return (await res.json()) as AuthStatus;
}

export async function login(email: string, password: string): Promise<AuthResponse> {
  const res = await fetch(`${getApiBase()}/system/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ email, password }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Login failed ${res.status}: ${text}`);
  }
  const data = (await res.json()) as AuthResponse;
  if (typeof localStorage !== 'undefined' && data.token) {
    localStorage.setItem('clawz_token', data.token);
  }
  return data;
}

export async function register(
  email: string,
  password: string,
  role?: string,
): Promise<AuthResponse & { api_key?: string }> {
  const res = await fetch(`${getApiBase()}/system/auth/register`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ email, password, role }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Register failed ${res.status}: ${text}`);
  }
  const data = (await res.json()) as AuthResponse & { api_key?: string };
  if (typeof localStorage !== 'undefined' && data.token) {
    localStorage.setItem('clawz_token', data.token);
  }
  return data;
}

export function clearAuthToken() {
  if (typeof localStorage !== 'undefined') {
    localStorage.removeItem('clawz_token');
    localStorage.removeItem('clawz_api_key');
  }
}

// ── System ─────────────────────────────────────────────────────────────────

export interface SystemHealth {
  status: string;
  uptime_secs?: number;
  version?: string;
  raw?: string;
}

/** Root liveness probe at `GET /health` (plain text OK). */
export async function fetchSystemHealth(): Promise<SystemHealth> {
  const res = await fetch(`${gatewayOrigin()}/health`, { headers: authHeaders() });
  const text = await res.text();
  if (!res.ok) {
    throw new Error(`Health ${res.status}: ${text.slice(0, 200)}`);
  }
  const trimmed = text.trim();
  if (trimmed === 'OK') {
    return { status: 'healthy', raw: trimmed };
  }
  try {
    const json = JSON.parse(trimmed) as Record<string, unknown>;
    return {
      status: String(json.status ?? 'unknown'),
      uptime_secs: json.uptime_secs != null ? Number(json.uptime_secs) : undefined,
      version: json.version != null ? String(json.version) : undefined,
      raw: trimmed,
    };
  } catch {
    return { status: trimmed || 'unknown', raw: trimmed };
  }
}

/** JSON health at `GET /api/v1/system/health`. */
export function fetchSystemHealthDetailed() {
  return req<{ status: string; uptime_secs: number; version: string }>('/system/health');
}

export interface SystemInfo {
  name: string;
  version: string;
  uptime_secs: number;
  stats: { agents: number; fleet_nodes: number };
  started_at?: string;
}

export function fetchSystemInfo() {
  return req<SystemInfo>('/system/info');
}

export interface SystemConfig {
  log_level: string;
  max_agents: number;
  enable_audit: boolean;
  gateway_version?: string;
}

export function fetchSystemConfig() {
  return req<SystemConfig>('/system/config');
}

export function resetSystemConfig() {
  return req<{ reset: boolean; config: SystemConfig }>('/system/config/reset', {
    method: 'POST',
  });
}

export async function fetchSystemMetrics(): Promise<string> {
  const res = await fetch(`${getApiBase()}/system/metrics`, { headers: authHeaders() });
  const text = await res.text();
  if (!res.ok) {
    throw new Error(`Metrics ${res.status}: ${text.slice(0, 200)}`);
  }
  return text;
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
  cloudflare: Record<string, boolean | string>;
  saas_connectors: {
    id: string;
    name: string;
    connected: boolean;
    icon?: string;
    env_hint?: string;
  }[];
  system: {
    port: number;
    jwt_secret_masked: string;
    log_level?: string;
    max_agents?: number;
    enable_audit?: boolean;
    auth_disabled?: boolean;
    gateway_version?: string;
  };
}

function normalizeProvider(raw: Record<string, unknown>): Provider {
  const providerType = String(raw.provider_type ?? raw.type ?? 'anthropic');
  return {
    id: String(raw.id ?? ''),
    name: String(raw.name ?? ''),
    type: providerType,
    api_key_masked:
      raw.api_key_masked != null ? String(raw.api_key_masked) : undefined,
    enabled: Boolean(raw.enabled ?? true),
    models: Array.isArray(raw.models) ? (raw.models as string[]) : undefined,
  };
}

export async function fetchConfig(): Promise<ConfigData> {
  const payload = await req<ConfigData | Record<string, unknown>>('/dashboard/config');
  const data = asRecord(payload);
  const system = asRecord(data.system);
  const cloudflare = asRecord(data.cloudflare);
  const cfToggles: Record<string, boolean> = {};
  for (const [key, value] of Object.entries(cloudflare)) {
    if (typeof value === 'boolean') {
      cfToggles[key] = value;
    }
  }
  return {
    providers: asArray<Record<string, unknown>>(data.providers).map(normalizeProvider),
    channels: asArray<Record<string, unknown>>(data.channels).map((ch) => ({
      id: String(ch.id ?? ''),
      name: String(ch.name ?? ''),
      type: String(ch.type ?? ch.channel_type ?? 'unknown'),
      status: (String(ch.status ?? 'idle') as Channel['status']) || 'idle',
      messages: Number(ch.messages ?? 0),
      enabled: Boolean(ch.enabled ?? true),
    })),
    default_provider: String(data.default_provider ?? 'none'),
    docker_registry: String(
      data.docker_registry ?? import.meta.env.VITE_DOCKER_REGISTRY ?? 'ghcr.io/improwyz',
    ),
    cloudflare: cfToggles,
    saas_connectors: asArray<Record<string, unknown>>(data.saas_connectors).map((s) => ({
      id: String(s.id ?? ''),
      name: String(s.name ?? s.id ?? ''),
      connected: Boolean(s.connected),
      icon: s.icon != null ? String(s.icon) : undefined,
      env_hint: s.env_hint != null ? String(s.env_hint) : undefined,
    })),
    system: {
      port: Number(system.port ?? import.meta.env.VITE_GATEWAY_PORT ?? 3000),
      jwt_secret_masked: String(system.jwt_secret_masked ?? '********'),
      log_level: system.log_level != null ? String(system.log_level) : undefined,
      max_agents: system.max_agents != null ? Number(system.max_agents) : undefined,
      enable_audit:
        system.enable_audit != null ? Boolean(system.enable_audit) : undefined,
      auth_disabled:
        system.auth_disabled != null ? Boolean(system.auth_disabled) : undefined,
      gateway_version:
        system.gateway_version != null ? String(system.gateway_version) : undefined,
    },
  };
}

export function updateProvider(id: string, data: Partial<Provider> & { api_key?: string }) {
  return req<Provider>(`/providers/${id}`, {
    method: 'PUT',
    body: JSON.stringify({
      name: data.name,
      provider_type: data.type,
      api_key: data.api_key || undefined,
      enabled: data.enabled,
    }),
  });
}

export function createProvider(data: Partial<Provider> & { api_key?: string }) {
  return req<Provider>('/providers', {
    method: 'POST',
    body: JSON.stringify({
      name: data.name,
      provider_type: data.type,
      api_key: data.api_key || undefined,
      enabled: data.enabled ?? true,
    }),
  });
}

export function updateSystemConfig(body: {
  log_level?: string;
  max_agents?: number;
  enable_audit?: boolean;
}) {
  return req<Record<string, unknown>>('/system/config', {
    method: 'PUT',
    body: JSON.stringify(body),
  });
}

export async function exportConfig() {
  return fetchConfig();
}

/** Cloudflare toggles are read from gateway env — not mutable at runtime yet. */
export async function updateCloudflare(_service: string, _enabled: boolean) {
  throw new Error(
    'Cloudflare services are configured via gateway environment variables (see docs).',
  );
}

/** SaaS connectors connect via env credentials until OAuth UI is implemented. */
export async function connectSaas(_id: string) {
  throw new Error(
    'Set CLAWZ_CONNECTOR_<NAME>_CONNECTED=1 and provider credentials in the gateway environment.',
  );
}

export async function disconnectSaas(_id: string) {
  throw new Error('Unset CLAWZ_CONNECTOR_<NAME>_CONNECTED on the gateway host.');
}

// ── Conversations ────────────────────────────────────────────────────────────

export interface Conversation {
  id: string;
  agent_id: string;
  room_id?: string;
  title?: string;
  archived?: boolean;
  message_count: number;
  created_at?: string;
  updated_at?: string;
}

export interface ConversationMessage {
  id?: string;
  role: string;
  content: string;
  created_at?: string;
}

function normalizeConversation(raw: Record<string, unknown>): Conversation {
  return {
    id: String(raw.id ?? ''),
    agent_id: String(raw.agent_id ?? ''),
    room_id: raw.room_id != null ? String(raw.room_id) : undefined,
    title: raw.title != null ? String(raw.title) : undefined,
    archived: raw.archived != null ? Boolean(raw.archived) : undefined,
    message_count: Number(raw.message_count ?? 0),
    created_at: raw.created_at != null ? String(raw.created_at) : undefined,
    updated_at: raw.updated_at != null ? String(raw.updated_at) : undefined,
  };
}

export async function listConversations(params?: {
  agent_id?: string;
  page?: number;
  limit?: number;
}) {
  const qs = new URLSearchParams();
  if (params?.agent_id) qs.set('agent_id', params.agent_id);
  if (params?.page != null) qs.set('page', String(params.page));
  if (params?.limit != null) qs.set('limit', String(params.limit));
  const q = qs.toString();
  const res = await req<unknown>(`/conversations${q ? `?${q}` : ''}`);
  return asArray<Record<string, unknown>>(res).map(normalizeConversation);
}

export function fetchConversations(params?: {
  agent_id?: string;
  page?: number;
  limit?: number;
}) {
  return listConversations(params);
}

export function fetchConversation(id: string) {
  return req<Conversation>(`/conversations/${id}`).then((c) =>
    normalizeConversation(asRecord(c)),
  );
}

export function createConversation(agentId: string, title?: string) {
  return req<Conversation>('/conversations', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId, title }),
  }).then((c) => normalizeConversation(asRecord(c)));
}

export function deleteConversation(id: string) {
  return req<void>(`/conversations/${id}`, { method: 'DELETE' });
}

export async function fetchConversationMessages(conversationId: string) {
  const res = await req<Record<string, unknown>>(`/conversations/${conversationId}/messages`);
  const messages = asArray<Record<string, unknown>>(res.messages ?? res.data).map(
    (m): ConversationMessage => ({
      id: m.id != null ? String(m.id) : undefined,
      role: String(m.role ?? 'user'),
      content: String(m.content ?? ''),
      created_at: m.created_at != null ? String(m.created_at) : undefined,
    }),
  );
  return { messages, total: Number(res.total ?? messages.length) };
}

export function sendConversationMessage(
  conversationId: string,
  content: string,
  role = 'user',
) {
  return req<Record<string, unknown>>(`/conversations/${conversationId}/messages`, {
    method: 'POST',
    body: JSON.stringify({ content, role }),
  });
}

export function archiveConversation(id: string) {
  return req<Record<string, unknown>>(`/conversations/${id}/archive`, { method: 'POST' });
}

// ── Rooms ────────────────────────────────────────────────────────────────────
export type RoomType = 'direct' | 'agent_team' | 'shared_agent';
export type ParticipantType = 'user' | 'agent';
export type ParticipantRole = 'owner' | 'member' | 'leader' | 'observer';
export type MessageKind =
  | 'user_text'
  | 'agent_text'
  | 'system'
  | 'delegation'
  | 'approval_request';
export type MessageVisibility = 'room' | 'private' | 'internal' | 'side_thread';

export interface RoomParticipant {
  participant_type: ParticipantType;
  participant_id: string;
  name?: string;
  role: ParticipantRole;
}

export interface Room {
  id: string;
  tenant_id?: string;
  title?: string;
  room_type: RoomType;
  orchestration_mode?: 'single' | 'team' | 'swarm';
  swarm_pattern?: string | null;
  parent_room_id?: string | null;
  visibility?: 'public' | 'private';
  participants: RoomParticipant[];
  primary_agent_id?: string;
  metadata?: Record<string, unknown>;
  created_at?: string;
}

export interface RoomMessage {
  id: string;
  room_id: string;
  seq: number;
  sender_type: ParticipantType | 'system';
  sender_id: string;
  sender_name?: string;
  content: string;
  message_kind?: MessageKind;
  visibility: MessageVisibility;
  target_agent_ids?: string[];
  thread_id?: string | null;
  timestamp?: string;
  created_at?: string;
}

/** Map gateway message records to UI-friendly shape. */
export function normalizeRoomMessage(msg: RoomMessage): RoomMessage {
  const kind =
    msg.message_kind ??
    (msg.sender_type === 'user'
      ? 'user_text'
      : msg.sender_type === 'agent'
        ? 'agent_text'
        : 'system');
  return {
    ...msg,
    message_kind: kind,
    timestamp: msg.timestamp ?? msg.created_at ?? new Date().toISOString(),
  };
}

export interface CreateRoomRequest {
  title?: string;
  room_type: RoomType;
  orchestration_mode?: 'single' | 'team' | 'swarm';
  participants: {
    participant_type: ParticipantType;
    participant_id: string;
    role?: ParticipantRole;
  }[];
  primary_agent_id?: string;
}

export interface SendRoomMessageRequest {
  content: string;
  client_message_id?: string;
  mentions?: string[];
  visibility?: MessageVisibility;
  thread_id?: string;
}

export interface CreateSideThreadRequest {
  title?: string;
  participant_ids?: string[];
}

export interface SideThread {
  id: string;
  room_id: string;
  title?: string;
  participant_ids: string[];
  created_by: string;
  created_at: string;
}

export interface OrchestrateRoomRequest {
  orchestration_mode?: string;
  config?: Record<string, unknown>;
  agent_ids?: string[];
}

export interface SendRoomMessageResponse {
  message: RoomMessage;
  response?: RoomMessage | null;
}

export interface PromoteMessageResponse {
  promoted: RoomMessage;
  source_message_id: string;
}

/** `GET /rooms` — gateway returns `{ "rooms": [...] }`. */
export async function listRooms() {
  const res = await req<unknown>('/rooms');
  return asArray<Room>(res);
}

export function fetchRooms() {
  return listRooms();
}

export function createRoom(data: CreateRoomRequest) {
  return req<Room>('/rooms', { method: 'POST', body: JSON.stringify(data) });
}

export function getRoom(id: string) {
  return req<Room>(`/rooms/${id}`);
}

export async function listRoomMessages(roomId: string, afterSeq?: number) {
  const qs = afterSeq != null ? `?after_seq=${afterSeq}` : '';
  const res = await req<{ messages: RoomMessage[]; has_more?: boolean }>(
    `/rooms/${roomId}/messages${qs}`,
  );
  return {
    ...res,
    messages: (res.messages ?? []).map(normalizeRoomMessage),
  };
}

export async function sendRoomMessage(roomId: string, data: SendRoomMessageRequest) {
  const res = await req<SendRoomMessageResponse>(`/rooms/${roomId}/messages`, {
    method: 'POST',
    body: JSON.stringify(data),
  });
  return {
    message: normalizeRoomMessage(res.message),
    response: res.response ? normalizeRoomMessage(res.response) : null,
  };
}

export async function createSideThread(roomId: string, data?: CreateSideThreadRequest) {
  const res = await req<{ side_thread: SideThread }>(`/rooms/${roomId}/side-threads`, {
    method: 'POST',
    body: JSON.stringify(data ?? {}),
  });
  return res.side_thread;
}

export function promoteMessage(roomId: string, messageId: string) {
  return req<PromoteMessageResponse>(`/rooms/${roomId}/messages/${messageId}/promote`, {
    method: 'POST',
  }).then((res) => ({
    ...res,
    promoted: normalizeRoomMessage(res.promoted),
  }));
}

export function orchestrateRoom(roomId: string, data?: OrchestrateRoomRequest) {
  return req<{ run_id: string }>(`/rooms/${roomId}/orchestrate`, {
    method: 'POST',
    body: JSON.stringify(data ?? {}),
  });
}

// ── Sessions ─────────────────────────────────────────────────────────────────

export interface SessionSummary {
  session_id: string;
  message_count: number;
  estimated_tokens: number;
}

export async function listSessions(): Promise<SessionSummary[]> {
  const payload = await req<{ data?: SessionSummary[] }>('/sessions');
  return asArray<SessionSummary>(payload);
}

/** Subscribe to SSE turn events for a single agent run (requires gateway auth headers). */
export function subscribeRunEvents(
  agentId: string,
  runId: string,
  onEvent: (data: unknown) => void,
  onError?: (err: Event) => void,
): () => void {
  const base = gatewayOrigin() || window.location.origin;
  const url = new URL(
    `${base}/api/v1/agents/${encodeURIComponent(agentId)}/runs/${encodeURIComponent(runId)}/events`,
  );
  const key = getStoredApiKey();
  if (key) url.searchParams.set('api_key', key);

  const es = new EventSource(url.toString());
  es.onmessage = (e) => {
    try {
      onEvent(JSON.parse(e.data));
    } catch {
      onEvent(e.data);
    }
  };
  es.onerror = (ev) => onError?.(ev);
  return () => es.close();
}

export async function compactSession(sessionId: string, keepLast = 40) {
  return req<{
    session_id: string;
    removed: number;
    message_count: number;
    estimated_tokens: number;
  }>(`/sessions/${encodeURIComponent(sessionId)}/compact`, {
    method: 'POST',
    body: JSON.stringify({ keep_last: keepLast }),
  });
}

// ── Workspace skills ──────────────────────────────────────────────────────────

export interface WorkspaceSkill {
  name: string;
  description?: string;
  path?: string;
  content?: string;
}

export async function fetchWorkspaceSkills(): Promise<{
  root?: string;
  skills: WorkspaceSkill[];
}> {
  const payload = await req<{ data?: WorkspaceSkill[]; root?: string }>('/skills');
  return { root: payload.root, skills: asArray<WorkspaceSkill>(payload) };
}

export async function createWorkspaceSkill(body: {
  name: string;
  content: string;
  description?: string;
}): Promise<WorkspaceSkill> {
  return req<WorkspaceSkill>('/skills', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

// ── Cron & background ───────────────────────────────────────────────────────

export interface CronJob {
  id: string;
  cron_expr: string;
  prompt: string;
  agent_id: string;
  enabled: boolean;
  name?: string | null;
  last_run_at?: string | null;
}

export async function fetchCronJobs(): Promise<CronJob[]> {
  const payload = await req<{ data?: CronJob[] }>('/cron/jobs');
  return asArray<CronJob>(payload);
}

export async function createCronJob(body: {
  cron_expr: string;
  prompt: string;
  agent_id?: string;
  name?: string;
  enabled?: boolean;
}): Promise<CronJob> {
  return req<CronJob>('/cron/jobs', { method: 'POST', body: JSON.stringify(body) });
}

export async function deleteCronJob(id: string): Promise<void> {
  await req<void>(`/cron/jobs/${id}`, { method: 'DELETE' });
}

export async function runCronJob(id: string): Promise<{ content: string }> {
  return req<{ content: string }>(`/cron/jobs/${id}/run`, { method: 'POST', body: '{}' });
}

export async function runSubconsciousTick(agentId?: string): Promise<{
  agent_id: string;
  conversation_id: string;
  content: string;
  chunks_reviewed: number;
}> {
  return req('/background/subconscious', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId ?? null }),
  });
}
