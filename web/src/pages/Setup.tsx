import { useCallback, useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { useLocation, useNavigate } from 'react-router-dom';
import { ClawzLogo } from '../components/shared/ClawzLogo';
import { Badge } from '../components/shared/Badge';
import {
  applySetup,
  completeSetup,
  completeSetupOAuth,
  detectWebPlatform,
  parseSetupOAuthReturn,
  fetchSetupStatus,
  setupStepToIndex,
  isDesktopWebPlatform,
  PLACEHOLDER_HOST_SPEC,
  runSetupDoctor,
  startSetupOAuth,
  startSetupSession,
  submitSetupAnswer,
  type DeploymentChoice,
  type DoctorCheck,
  type HostSpecReport,
  type InstallStrategy,
  type OAuthProvider,
  type SetupSecrets,
} from '../lib/setup';

const STEPS = [
  'Welcome',
  'Deploy',
  'Install',
  'Stack',
  'Secrets',
  'LLM',
  'Identity',
  'Skills',
  'Topology',
  'Verify',
  'Done',
] as const;

const SKILL_OPTIONS = [
  { id: 'code-review', label: 'Code review', defaultOn: true },
  { id: 'debugging', label: 'Debugging', defaultOn: false },
  { id: 'api-design', label: 'API design', defaultOn: false },
  { id: 'security', label: 'Security audit', defaultOn: false },
  { id: 'docs', label: 'Documentation', defaultOn: false },
] as const;

interface WizardAnswers {
  oauth_provider?: OAuthProvider;
  deployment?: DeploymentChoice;
  install_strategy?: InstallStrategy;
  anthropic_key: string;
  openai_key: string;
  cursor_key: string;
  llm_provider: string;
  agent_name: string;
  agent_who: string;
  agent_role: string;
  agent_tone: string;
  skills: string[];
  topology: 'single' | 'orchestrator_worker';
}

const defaultAnswers = (): WizardAnswers => ({
  anthropic_key: '',
  openai_key: '',
  cursor_key: '',
  llm_provider: 'anthropic',
  agent_name: 'clawz',
  agent_who: 'A capable ClawZ assistant',
  agent_role: 'Help the operator run and govern AI agents safely.',
  agent_tone: 'Clear, concise, and proactive.',
  skills: SKILL_OPTIONS.filter((s) => s.defaultOn).map((s) => s.id),
  topology: 'single',
});

function maskSecret(value: string | undefined, visible = 4): string {
  if (!value) return '—';
  if (value.length <= visible * 2) return '••••••••';
  return `${value.slice(0, visible)}…${value.slice(-visible)}`;
}

function Stepper({ step }: { step: number }) {
  return (
    <nav className="flex flex-wrap gap-1 mb-6" aria-label="Setup progress">
      {STEPS.map((label, i) => {
        const active = i === step;
        const done = i < step;
        return (
          <div
            key={label}
            className={`flex items-center gap-1 px-2 py-1 rounded text-[10px] sm:text-xs font-medium ${
              active
                ? 'bg-blue-600/20 text-blue-400 border border-blue-600/40'
                : done
                  ? 'text-zinc-400'
                  : 'text-zinc-600'
            }`}
          >
            <span
              className={`w-4 h-4 rounded-full flex items-center justify-center text-[9px] ${
                active ? 'bg-blue-600 text-white' : done ? 'bg-zinc-600 text-zinc-200' : 'bg-zinc-800'
              }`}
            >
              {done ? '✓' : i}
            </span>
            <span className="hidden sm:inline">{label}</span>
          </div>
        );
      })}
    </nav>
  );
}

function HostSpecCards({ spec }: { spec: HostSpecReport }) {
  const cards = [
    { label: 'OS', value: spec.os },
    { label: 'Arch', value: spec.arch },
    { label: 'RAM', value: spec.ram_mb != null ? `${spec.ram_mb} MB` : '—' },
    { label: 'Disk free', value: spec.disk_free_gb != null ? `${spec.disk_free_gb} GB` : '—' },
    { label: 'CPUs', value: spec.cpu_count?.toString() ?? '—' },
    {
      label: 'Docker',
      value: spec.docker_available ? 'Available' : 'Missing',
      warn: !spec.docker_available,
    },
  ];
  return (
    <div className="space-y-3">
      <p className="text-zinc-400 text-sm">{spec.summary}</p>
      <div className="grid grid-cols-2 sm:grid-cols-3 gap-2">
        {cards.map((c) => (
          <div
            key={c.label}
            className={`p-3 rounded-lg border ${
              c.warn ? 'bg-amber-950/30 border-amber-800' : 'bg-zinc-800 border-zinc-700'
            }`}
          >
            <div className="text-zinc-500 text-[10px] uppercase tracking-wider">{c.label}</div>
            <div className="text-zinc-100 text-sm font-medium mt-0.5">{c.value}</div>
          </div>
        ))}
      </div>
      {spec.warnings.length > 0 && (
        <ul className="space-y-1 text-amber-400/90 text-xs list-disc list-inside">
          {spec.warnings.map((w) => (
            <li key={w}>{w}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

function DesktopPlatformPanel() {
  const platform = detectWebPlatform();
  if (!isDesktopWebPlatform()) return null;
  const isWin = platform === 'windows';
  return (
    <div className="mb-4 p-4 rounded-lg bg-blue-950/30 border border-blue-800/50 space-y-2">
      <h3 className="text-blue-300 text-sm font-medium">
        {isWin ? 'Windows' : 'macOS'} — connect to a remote gateway
      </h3>
      <p className="text-zinc-400 text-xs leading-relaxed">
        ClawZ micro/fleet stacks run best on Linux with Docker. On {isWin ? 'Windows' : 'macOS'}, point
        this wizard at a{' '}
        <strong className="text-zinc-300">remote gateway URL</strong> (VPS or homelab) or use{' '}
        {isWin ? (
          <a
            href="https://learn.microsoft.com/en-us/windows/wsl/install"
            target="_blank"
            rel="noopener noreferrer"
            className="text-blue-400 hover:underline"
          >
            WSL2
          </a>
        ) : (
          'Docker Desktop'
        )}{' '}
        for local containers. Set <code className="text-zinc-400">VITE_API_BASE</code> to your gateway
        before finishing setup.
      </p>
    </div>
  );
}

function OAuthPanel({
  selected,
  onSelect,
  busy,
}: {
  selected?: OAuthProvider;
  onSelect: (p: OAuthProvider) => void;
  busy: boolean;
}) {
  const providers: { id: OAuthProvider; label: string }[] = [
    { id: 'cursor', label: 'Cursor' },
    { id: 'codex', label: 'Codex' },
    { id: 'anthropic', label: 'Anthropic' },
    { id: 'openai', label: 'OpenAI' },
    { id: 'skip', label: 'Skip — manual keys' },
  ];
  return (
    <div className="space-y-2">
      <p className="text-zinc-500 text-xs">
        Optional: sign in to power the setup assistant. You can always enter API keys manually later.
      </p>
      <div className="flex flex-wrap gap-2">
        {providers.map((p) => (
          <button
            key={p.id}
            type="button"
            disabled={busy}
            onClick={() => onSelect(p.id)}
            className={`px-3 py-1.5 rounded-lg text-sm border transition-colors disabled:opacity-50 ${
              selected === p.id
                ? 'bg-blue-600 border-blue-500 text-white'
                : 'bg-zinc-800 border-zinc-700 text-zinc-300 hover:border-zinc-500'
            }`}
          >
            {p.label}
          </button>
        ))}
      </div>
    </div>
  );
}

function ChoiceCard({
  title,
  description,
  selected,
  onClick,
}: {
  title: string;
  description: string;
  selected: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`text-left p-4 rounded-xl border transition-colors w-full ${
        selected
          ? 'border-blue-500 bg-blue-600/10 ring-1 ring-blue-500/50'
          : 'border-zinc-700 bg-zinc-800 hover:border-zinc-600'
      }`}
    >
      <div className="text-zinc-100 font-medium text-sm">{title}</div>
      <div className="text-zinc-500 text-xs mt-1">{description}</div>
    </button>
  );
}

function SecretsPanel({
  secrets,
  confirmed,
  onConfirmChange,
  onApply,
  applying,
}: {
  secrets: SetupSecrets | null;
  confirmed: boolean;
  onConfirmChange: (v: boolean) => void;
  onApply: () => void;
  applying: boolean;
}) {
  const rows = [
    { label: 'JWT secret', raw: secrets?.jwt_secret, masked: secrets?.jwt_secret_masked },
    ...(secrets?.api_keys_masked ?? secrets?.api_keys ?? []).map((k, i) => ({
      label: `API key ${i + 1}`,
      raw: secrets?.api_keys?.[i],
      masked: typeof k === 'string' ? k : undefined,
    })),
    {
      label: 'Worker token',
      raw: secrets?.worker_token,
      masked: secrets?.worker_token_masked,
    },
  ].filter((r) => r.masked || r.raw);

  if (rows.length === 0) {
    return (
      <p className="text-zinc-500 text-sm">
        Continue to generate platform secrets on the gateway, or apply locally with{' '}
        <code className="text-zinc-400">clawz onboard</code>.
      </p>
    );
  }

  return (
    <div className="space-y-3">
      {rows.map((r) => (
        <div key={r.label} className="p-3 rounded-lg bg-zinc-800 border border-zinc-700">
          <div className="text-zinc-500 text-xs">{r.label}</div>
          <div className="text-zinc-200 font-mono text-sm mt-1">
            {r.masked ?? maskSecret(r.raw)}
          </div>
        </div>
      ))}
      <label className="flex items-center gap-2 cursor-pointer text-sm text-zinc-300">
        <input
          type="checkbox"
          checked={confirmed}
          onChange={(e) => onConfirmChange(e.target.checked)}
        />
        I have saved these secrets
      </label>
      <button
        type="button"
        disabled={applying || !confirmed}
        onClick={onApply}
        className="px-3 py-1.5 text-sm bg-blue-600 text-white rounded-lg hover:bg-blue-500 disabled:opacity-50"
      >
        {applying ? 'Applying…' : 'Apply secrets'}
      </button>
    </div>
  );
}

function DoctorPanel({
  checks,
  loading,
  onRetry,
}: {
  checks: DoctorCheck[];
  loading: boolean;
  onRetry: () => void;
}) {
  return (
    <div className="space-y-2">
      {checks.length === 0 && !loading && (
        <p className="text-zinc-500 text-sm">Run verification to execute doctor checks.</p>
      )}
      {checks.map((c) => (
        <div
          key={c.name}
          className={`flex items-start gap-3 p-3 rounded-lg border ${
            c.ok ? 'bg-zinc-800 border-zinc-700' : 'bg-red-950/20 border-red-800/50'
          }`}
        >
          <Badge variant={c.ok ? 'success' : 'error'}>{c.ok ? 'pass' : 'fail'}</Badge>
          <div className="min-w-0 flex-1">
            <div className="text-zinc-200 text-sm font-medium">{c.name}</div>
            <div className="text-zinc-500 text-xs mt-0.5">{c.detail}</div>
            {c.remediation && !c.ok && (
              <div className="text-amber-400/80 text-xs mt-1">{c.remediation}</div>
            )}
          </div>
        </div>
      ))}
      <button
        type="button"
        onClick={onRetry}
        disabled={loading}
        className="px-3 py-1.5 text-sm bg-zinc-700 text-zinc-200 rounded-lg hover:bg-zinc-600 disabled:opacity-50"
      >
        {loading ? 'Running checks…' : 'Retry doctor'}
      </button>
    </div>
  );
}

export function Setup() {
  const navigate = useNavigate();
  const location = useLocation();
  const [step, setStep] = useState(0);
  const [answers, setAnswers] = useState<WizardAnswers>(defaultAnswers);
  const [secrets, setSecrets] = useState<SetupSecrets | null>(null);
  const [secretsConfirmed, setSecretsConfirmed] = useState(false);
  const [doctorChecks, setDoctorChecks] = useState<DoctorCheck[]>([]);
  const [error, setError] = useState('');
  const [oauthMessage, setOauthMessage] = useState('');

  const { data: status, isLoading: statusLoading } = useQuery({
    queryKey: ['setup-status'],
    queryFn: fetchSetupStatus,
    staleTime: 10_000,
  });

  const hostSpec = status?.host_spec ?? PLACEHOLDER_HOST_SPEC;

  useEffect(() => {
    if (status?.setup_complete) {
      navigate('/dashboard', { replace: true });
    }
    if (status?.step) {
      const idx = setupStepToIndex(status.step);
      if (idx > 0 && idx <= 10) setStep(idx);
    }
  }, [status, navigate]);

  useEffect(() => {
    startSetupSession(detectWebPlatform()).catch(() => {
      /* gateway route may not exist yet */
    });
  }, []);

  useEffect(() => {
    const params = new URLSearchParams(location.search);
    if (params.get('oauth') === 'success') {
      setOauthMessage('Sign-in complete. Continue with the wizard.');
      navigate('/setup', { replace: true });
      return;
    }
    const ret = parseSetupOAuthReturn(location.search);
    if (!ret) return;
    completeSetupOAuth({ state: ret.state, code: ret.code })
      .then((res) => {
        setAnswers((a) => ({ ...a, oauth_provider: res.provider as OAuthProvider }));
        setOauthMessage(`Signed in with ${res.provider}.`);
        setError('');
        navigate('/setup', { replace: true });
      })
      .catch((e) => setError(String(e)));
  }, [location.search, navigate]);

  const syncAnswer = useCallback(
    async (payload: Parameters<typeof submitSetupAnswer>[0]) => {
      try {
        const res = await submitSetupAnswer(payload);
        if (res.advanced_to) setStep(setupStepToIndex(res.advanced_to));
        else if (res.step) setStep(setupStepToIndex(res.step));
        return res;
      } catch {
        return null;
      }
    },
    [],
  );

  const oauthMut = useMutation({
    mutationFn: (provider: OAuthProvider) => startSetupOAuth(provider),
    onSuccess: (data, provider) => {
      setAnswers((a) => ({ ...a, oauth_provider: provider }));
      if (data.flow === 'imported') {
        setOauthMessage(data.message);
      } else if (data.device_code) {
        setOauthMessage(
          `${data.message} Open ${data.verification_uri ?? 'https://auth.openai.com/codex/device'} and enter: ${data.device_code}`,
        );
        if (data.auth_url) window.open(data.auth_url, '_blank', 'noopener,noreferrer');
      } else if (data.flow === 'api_key') {
        setOauthMessage(data.message);
        if (data.auth_url) window.open(data.auth_url, '_blank', 'noopener,noreferrer');
      } else if (data.auth_url) {
        window.open(data.auth_url, '_blank', 'noopener,noreferrer');
        setOauthMessage('Complete sign-in in the browser tab. You will return here automatically.');
      } else {
        setOauthMessage(data.message ?? (provider === 'skip' ? 'Manual path selected.' : 'OAuth started.'));
      }
      setError('');
    },
    onError: (e) => {
      setAnswers((a) => ({ ...a, oauth_provider: 'skip' }));
      setOauthMessage('OAuth API not available — continue with manual API keys.');
      setError(String(e));
    },
  });

  const applyMut = useMutation({
    mutationFn: () => applySetup(),
    onSuccess: () => {
      setSecretsConfirmed(true);
      setError('');
    },
    onError: (e) => setError(String(e)),
  });

  const doctorMut = useMutation({
    mutationFn: runSetupDoctor,
    onSuccess: ({ checks }) => {
      setDoctorChecks(checks);
      setError('');
    },
    onError: (e) => setError(String(e)),
  });

  const completeMut = useMutation({
    mutationFn: completeSetup,
    onSuccess: () => navigate('/dashboard', { replace: true }),
    onError: () => navigate('/dashboard', { replace: true }),
  });

  const goNext = useCallback(async () => {
    setError('');
    const next = Math.min(step + 1, 10);

    if (step === 2 && answers.install_strategy) {
      await syncAnswer({
        install_strategy: answers.install_strategy,
        advance: true,
      });
    }
    if (step === 3) {
      await syncAnswer({ advance: true });
      setStep(4);
      return;
    }
    if (step === 4) {
      if (!secrets) {
        setSecrets({
          jwt_secret_masked: 'clawz-jwt-••••••••',
          api_keys_masked: ['clawz-dev-••••••••'],
        });
        return;
      }
      if (!secretsConfirmed) return;
      await syncAnswer({ advance: true });
    }
    if (step === 5) {
      const llmKey =
        answers.llm_provider === 'openai'
          ? answers.openai_key
          : answers.llm_provider === 'anthropic'
            ? answers.anthropic_key
            : answers.cursor_key;
      await syncAnswer({
        llm_provider: answers.llm_provider,
        llm_api_key: llmKey || undefined,
        advance: true,
      });
    }
    if (step === 6) {
      await syncAnswer({
        identity_name: answers.agent_name,
        identity_who_am_i: answers.agent_who,
        identity_role: answers.agent_role,
        advance: true,
      });
    }
    if (step === 7) {
      await syncAnswer({ advance: true });
    }
    if (step === 8 && answers.deployment) {
      await syncAnswer({
        deployment: answers.deployment,
        advance: true,
      });
    }
    if (step === 9) {
      if (doctorChecks.length === 0 || doctorMut.isPending) {
        doctorMut.mutate(undefined, {
          onSuccess: ({ checks, all_ok }) => {
            if (all_ok) setStep(10);
          },
        });
        return;
      }
      if (doctorChecks.every((c) => c.ok)) {
        setStep(10);
      }
      return;
    }

    setStep(next);
    if (step === 1 && answers.deployment) {
      await syncAnswer({ deployment: answers.deployment, advance: true });
    }
  }, [step, answers, secrets, doctorChecks, syncAnswer, doctorMut]);

  const goBack = () => {
    setError('');
    setStep((s) => Math.max(0, s - 1));
  };

  const deployment = answers.deployment;
  const topologyLocked = deployment === 'standalone';

  const stepTitle = STEPS[step] ?? 'Setup';

  const canContinue = useMemo(() => {
    switch (step) {
      case 1:
        return Boolean(answers.deployment);
      case 2:
        return Boolean(answers.install_strategy);
      case 4:
        if (!secrets) return true;
        return secretsConfirmed;
      case 5:
        return Boolean(answers.llm_provider);
      case 6:
        return answers.agent_name.trim().length > 0;
      case 9:
        return doctorChecks.length > 0 && doctorChecks.every((c) => c.ok);
      default:
        return true;
    }
  }, [step, answers, secrets, secretsConfirmed, doctorChecks]);

  useEffect(() => {
    if (step === 3) {
      const t = window.setTimeout(() => {
        setStep(4);
        syncAnswer({ step: 3, field: 'stack', value: 'auto' });
      }, 2200);
      return () => window.clearTimeout(t);
    }
    return undefined;
  }, [step, syncAnswer]);

  useEffect(() => {
    if (topologyLocked) {
      setAnswers((a) => ({ ...a, topology: 'single' }));
    } else if (answers.topology === 'single' && deployment && deployment !== 'standalone') {
      setAnswers((a) => ({ ...a, topology: 'orchestrator_worker' }));
    }
  }, [topologyLocked, deployment, answers.topology]);

  return (
    <div className="min-h-screen bg-zinc-950 flex flex-col">
      <header className="border-b border-zinc-800 px-4 py-4 flex items-center justify-between">
        <div className="flex items-center gap-3">
          <ClawzLogo variant="full" theme="dark" className="h-7 w-auto" />
          <span className="text-zinc-500 text-sm hidden sm:inline">First-run setup</span>
        </div>
        <Badge variant="default">{detectWebPlatform()}</Badge>
      </header>

      <main className="flex-1 overflow-y-auto p-4 max-w-3xl mx-auto w-full">
        <DesktopPlatformPanel />

        <div className="widget-3d rounded-xl bg-zinc-900 border border-zinc-800 p-4 sm:p-6">
          <Stepper step={step} />
          <h1 className="text-zinc-100 text-lg font-semibold mb-1">{stepTitle}</h1>
          <p className="text-zinc-500 text-xs mb-4">Phase {step} of 10</p>

          {error && (
            <div className="mb-4 px-3 py-2 bg-red-900/30 border border-red-700 rounded text-red-400 text-sm">
              {error}
            </div>
          )}

          {statusLoading && step === 0 && (
            <div className="h-24 bg-zinc-800 rounded-lg animate-pulse mb-4" />
          )}

          {step === 0 && (
            <div className="space-y-4">
              <p className="text-zinc-400 text-sm">
                Welcome to ClawZ. We will configure deployment mode, secrets, your first agent, and run
                doctor checks before opening the dashboard.
              </p>
              <OAuthPanel
                selected={answers.oauth_provider}
                onSelect={(p) => oauthMut.mutate(p)}
                busy={oauthMut.isPending}
              />
              {oauthMessage && <p className="text-blue-400/90 text-xs">{oauthMessage}</p>}
              <HostSpecCards spec={hostSpec} />
            </div>
          )}

          {step === 1 && (
            <div className="grid gap-3 sm:grid-cols-3">
              <ChoiceCard
                title="Standalone"
                description="Single binary, SQLite or local Postgres. Best for dev and small teams."
                selected={answers.deployment === 'standalone'}
                onClick={() => setAnswers((a) => ({ ...a, deployment: 'standalone' }))}
              />
              <ChoiceCard
                title="Micro"
                description="Docker Compose fleet: gateway + worker + Postgres."
                selected={answers.deployment === 'micro'}
                onClick={() => setAnswers((a) => ({ ...a, deployment: 'micro' }))}
              />
              <ChoiceCard
                title="Elastic"
                description="Full mesh, leader election, dynamic discovery."
                selected={answers.deployment === 'elastic'}
                onClick={() => setAnswers((a) => ({ ...a, deployment: 'elastic' }))}
              />
            </div>
          )}

          {step === 2 && (
            <div className="grid gap-3">
              <ChoiceCard
                title="Prebuilt (GHCR)"
                description="Pull gateway/worker images from GitHub Container Registry. Fastest path."
                selected={answers.install_strategy === 'prebuilt'}
                onClick={() => setAnswers((a) => ({ ...a, install_strategy: 'prebuilt' }))}
              />
              <ChoiceCard
                title="Build locally"
                description="docker compose with build overlay. Needs Docker and disk space."
                selected={answers.install_strategy === 'build'}
                onClick={() => setAnswers((a) => ({ ...a, install_strategy: 'build' }))}
              />
              <ChoiceCard
                title="Source (cargo)"
                description="Compile Rust workspace on this machine. Requires Rust 1.87+."
                selected={answers.install_strategy === 'source'}
                onClick={() => setAnswers((a) => ({ ...a, install_strategy: 'source' }))}
              />
            </div>
          )}

          {step === 3 && (
            <div className="py-8 text-center space-y-3">
              <div className="inline-block w-8 h-8 border-2 border-blue-500 border-t-transparent rounded-full animate-spin" />
              <p className="text-zinc-300 text-sm">Preparing stack…</p>
              <p className="text-zinc-500 text-xs max-w-md mx-auto">
                Compose up, migrations, and health probes will run on the gateway when setup API is
                wired. Advancing automatically.
              </p>
            </div>
          )}

          {step === 4 && (
            <SecretsPanel
              secrets={secrets}
              confirmed={secretsConfirmed}
              applying={applyMut.isPending}
              onConfirmChange={setSecretsConfirmed}
              onApply={() => applyMut.mutate()}
            />
          )}

          {step === 5 && (
            <div className="space-y-3">
              <div>
                <label className="block text-zinc-400 text-xs mb-1">Primary LLM provider</label>
                <select
                  className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
                  value={answers.llm_provider}
                  onChange={(e) => setAnswers((a) => ({ ...a, llm_provider: e.target.value }))}
                >
                  <option value="anthropic">Anthropic</option>
                  <option value="openai">OpenAI</option>
                  <option value="cursor">Cursor</option>
                </select>
              </div>
              <div>
                <label className="block text-zinc-400 text-xs mb-1">Anthropic API key</label>
                <input
                  type="password"
                  className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 font-mono"
                  placeholder="sk-ant-…"
                  value={answers.anthropic_key}
                  onChange={(e) => setAnswers((a) => ({ ...a, anthropic_key: e.target.value }))}
                />
              </div>
              <div>
                <label className="block text-zinc-400 text-xs mb-1">OpenAI API key</label>
                <input
                  type="password"
                  className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 font-mono"
                  placeholder="sk-…"
                  value={answers.openai_key}
                  onChange={(e) => setAnswers((a) => ({ ...a, openai_key: e.target.value }))}
                />
              </div>
              <div>
                <label className="block text-zinc-400 text-xs mb-1">Cursor API key</label>
                <input
                  type="password"
                  className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 font-mono"
                  placeholder="key_…"
                  value={answers.cursor_key}
                  onChange={(e) => setAnswers((a) => ({ ...a, cursor_key: e.target.value }))}
                />
              </div>
            </div>
          )}

          {step === 6 && (
            <div className="space-y-3">
              <div>
                <label className="block text-zinc-400 text-xs mb-1">Agent name</label>
                <input
                  className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2"
                  value={answers.agent_name}
                  onChange={(e) => setAnswers((a) => ({ ...a, agent_name: e.target.value }))}
                />
              </div>
              <div>
                <label className="block text-zinc-400 text-xs mb-1">Who am I?</label>
                <textarea
                  className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 min-h-[72px]"
                  value={answers.agent_who}
                  onChange={(e) => setAnswers((a) => ({ ...a, agent_who: e.target.value }))}
                />
              </div>
              <div>
                <label className="block text-zinc-400 text-xs mb-1">Role (system prompt)</label>
                <textarea
                  className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 min-h-[96px]"
                  value={answers.agent_role}
                  onChange={(e) => setAnswers((a) => ({ ...a, agent_role: e.target.value }))}
                />
              </div>
              <div>
                <label className="block text-zinc-400 text-xs mb-1">Tone</label>
                <textarea
                  className="w-full bg-zinc-800 border border-zinc-700 text-zinc-100 text-sm rounded-lg px-3 py-2 min-h-[64px]"
                  value={answers.agent_tone}
                  onChange={(e) => setAnswers((a) => ({ ...a, agent_tone: e.target.value }))}
                />
              </div>
            </div>
          )}

          {step === 7 && (
            <div className="space-y-2">
              {SKILL_OPTIONS.map((s) => (
                <label
                  key={s.id}
                  className="flex items-center gap-3 p-3 rounded-lg bg-zinc-800 border border-zinc-700 cursor-pointer"
                >
                  <input
                    type="checkbox"
                    checked={answers.skills.includes(s.id)}
                    onChange={() =>
                      setAnswers((a) => ({
                        ...a,
                        skills: a.skills.includes(s.id)
                          ? a.skills.filter((x) => x !== s.id)
                          : [...a.skills, s.id],
                      }))
                    }
                  />
                  <span className="text-zinc-200 text-sm">{s.label}</span>
                </label>
              ))}
            </div>
          )}

          {step === 8 && (
            <div className="grid gap-3">
              <ChoiceCard
                title="Single agent"
                description="One primary agent with full tool access."
                selected={answers.topology === 'single'}
                onClick={() => setAnswers((a) => ({ ...a, topology: 'single' }))}
              />
              <ChoiceCard
                title="Orchestrator + worker"
                description="Planner agent plus executor worker (recommended for micro/elastic)."
                selected={answers.topology === 'orchestrator_worker'}
                onClick={() =>
                  !topologyLocked && setAnswers((a) => ({ ...a, topology: 'orchestrator_worker' }))
                }
              />
              {topologyLocked && (
                <p className="text-zinc-500 text-xs">Standalone mode uses a single agent topology.</p>
              )}
            </div>
          )}

          {step === 9 && (
            <DoctorPanel
              checks={doctorChecks}
              loading={doctorMut.isPending}
              onRetry={() => doctorMut.mutate()}
            />
          )}

          {step === 10 && (
            <div className="text-center py-6 space-y-4">
              <div className="text-4xl">✓</div>
              <h2 className="text-zinc-100 font-semibold">Setup complete</h2>
              <p className="text-zinc-500 text-sm max-w-md mx-auto">
                ClawZ is ready. Open the dashboard to manage agents, governance, and fleet health.
              </p>
              <button
                type="button"
                onClick={() => completeMut.mutate()}
                disabled={completeMut.isPending}
                className="px-4 py-2 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-500 disabled:opacity-50"
              >
                {completeMut.isPending ? 'Finishing…' : 'Go to dashboard'}
              </button>
            </div>
          )}

          {step < 10 && step !== 3 && (
            <div className="flex justify-between mt-6 pt-4 border-t border-zinc-800">
              <button
                type="button"
                onClick={goBack}
                disabled={step === 0}
                className="px-3 py-1.5 text-sm rounded-lg bg-zinc-800 text-zinc-400 hover:text-zinc-200 disabled:opacity-40"
              >
                Back
              </button>
              <button
                type="button"
                onClick={() => void goNext()}
                disabled={!canContinue}
                className="px-4 py-1.5 text-sm rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50"
              >
                {step === 9 ? 'Verify & continue' : 'Continue'}
              </button>
            </div>
          )}
        </div>
      </main>
    </div>
  );
}
