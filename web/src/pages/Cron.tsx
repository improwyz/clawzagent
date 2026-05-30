import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { Badge } from '../components/shared/Badge';
import { EmptyState } from '../components/shared/EmptyState';
import { Modal } from '../components/shared/Modal';
import {
  createCronJob,
  deleteCronJob,
  fetchCronJobs,
  runCronJob,
  runSubconsciousTick,
  type CronJob,
} from '../lib/api';

export function Cron() {
  const qc = useQueryClient();
  const [addOpen, setAddOpen] = useState(false);
  const [form, setForm] = useState({ cron_expr: '0 9 * * *', prompt: '', agent_id: '', name: '' });
  const [runOutput, setRunOutput] = useState<string | null>(null);

  const { data: jobs = [], isLoading, error } = useQuery({
    queryKey: ['cron-jobs'],
    queryFn: fetchCronJobs,
  });

  const createMut = useMutation({
    mutationFn: () =>
      createCronJob({
        cron_expr: form.cron_expr,
        prompt: form.prompt,
        agent_id: form.agent_id || undefined,
        name: form.name || undefined,
        enabled: true,
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['cron-jobs'] });
      setAddOpen(false);
      setForm({ cron_expr: '0 9 * * *', prompt: '', agent_id: '', name: '' });
    },
  });

  const runMut = useMutation({
    mutationFn: (id: string) => runCronJob(id),
    onSuccess: (res) => setRunOutput(res.content),
  });

  const deleteMut = useMutation({
    mutationFn: (id: string) => deleteCronJob(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['cron-jobs'] }),
  });

  const subconsciousMut = useMutation({
    mutationFn: () => runSubconsciousTick(),
    onSuccess: (res) =>
      setRunOutput(`Subconscious (${res.chunks_reviewed} chunks):\n${res.content}`),
  });

  return (
    <div className="p-6 space-y-6 max-w-5xl">
      <div className="flex items-center justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold text-zinc-100">Scheduled jobs</h1>
          <p className="text-sm text-zinc-500 mt-1">
            Cron agents and background subconscious ticks (tools disabled during runs).
          </p>
        </div>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => subconsciousMut.mutate()}
            disabled={subconsciousMut.isPending}
            className="px-3 py-1.5 text-sm rounded-lg border border-zinc-700 text-zinc-300 hover:bg-zinc-800"
          >
            Run subconscious
          </button>
          <button
            type="button"
            onClick={() => setAddOpen(true)}
            className="px-3 py-1.5 text-sm rounded-lg bg-blue-600 text-white hover:bg-blue-500"
          >
            Add job
          </button>
        </div>
      </div>

      {runOutput && (
        <pre className="text-xs text-zinc-300 bg-zinc-900 border border-zinc-800 rounded-lg p-4 whitespace-pre-wrap">
          {runOutput}
        </pre>
      )}

      {isLoading && <p className="text-zinc-500 text-sm">Loading jobs…</p>}
      {error && <p className="text-red-400 text-sm">{String(error)}</p>}

      {!isLoading && jobs.length === 0 && (
        <EmptyState
          title="No cron jobs"
          description='Add a schedule with clawz cron add or the button above.'
        />
      )}

      {jobs.length > 0 && (
        <div className="space-y-2">
          {jobs.map((job: CronJob) => (
            <div
              key={job.id}
              className="widget-3d flex items-center justify-between gap-4 p-4 rounded-xl bg-zinc-900 border border-zinc-800"
            >
              <div className="min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="text-zinc-100 font-medium font-mono text-sm">{job.cron_expr}</span>
                  <Badge variant={job.enabled ? 'success' : 'default'}>
                    {job.enabled ? 'enabled' : 'disabled'}
                  </Badge>
                </div>
                <p className="text-zinc-400 text-sm mt-1 truncate">{job.prompt}</p>
                <p className="text-zinc-600 text-xs mt-1">
                  agent={job.agent_id}
                  {job.last_run_at ? ` · last run ${job.last_run_at}` : ''}
                </p>
              </div>
              <div className="flex gap-2 flex-shrink-0">
                <button
                  type="button"
                  onClick={() => runMut.mutate(job.id)}
                  disabled={runMut.isPending}
                  className="px-2.5 py-1 text-xs rounded-md bg-zinc-800 text-zinc-200 hover:bg-zinc-700"
                >
                  Run now
                </button>
                <button
                  type="button"
                  onClick={() => deleteMut.mutate(job.id)}
                  disabled={deleteMut.isPending}
                  className="px-2.5 py-1 text-xs rounded-md border border-zinc-700 text-zinc-400 hover:text-red-300"
                >
                  Delete
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      <Modal open={addOpen} onClose={() => setAddOpen(false)} title="Add cron job">
        <div className="space-y-3">
          <label className="block text-xs text-zinc-500">
            Cron expression (5-field)
            <input
              className="mt-1 w-full bg-zinc-800 border border-zinc-700 rounded-lg px-3 py-2 text-sm text-zinc-100"
              value={form.cron_expr}
              onChange={(e) => setForm({ ...form, cron_expr: e.target.value })}
            />
          </label>
          <label className="block text-xs text-zinc-500">
            Prompt
            <textarea
              className="mt-1 w-full bg-zinc-800 border border-zinc-700 rounded-lg px-3 py-2 text-sm text-zinc-100 min-h-[80px]"
              value={form.prompt}
              onChange={(e) => setForm({ ...form, prompt: e.target.value })}
            />
          </label>
          <label className="block text-xs text-zinc-500">
            Agent ID (optional)
            <input
              className="mt-1 w-full bg-zinc-800 border border-zinc-700 rounded-lg px-3 py-2 text-sm text-zinc-100"
              value={form.agent_id}
              onChange={(e) => setForm({ ...form, agent_id: e.target.value })}
            />
          </label>
          <label className="block text-xs text-zinc-500">
            Name (optional)
            <input
              className="mt-1 w-full bg-zinc-800 border border-zinc-700 rounded-lg px-3 py-2 text-sm text-zinc-100"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
            />
          </label>
          <button
            type="button"
            onClick={() => createMut.mutate()}
            disabled={createMut.isPending || !form.prompt.trim()}
            className="w-full py-2 rounded-lg bg-blue-600 text-white text-sm hover:bg-blue-500 disabled:opacity-50"
          >
            Create
          </button>
        </div>
      </Modal>
    </div>
  );
}
