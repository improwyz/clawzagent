interface EmptyStateProps {
  icon?: string;
  title: string;
  description?: string;
  action?: React.ReactNode;
}

export function EmptyState({ icon = '📭', title, description, action }: EmptyStateProps) {
  return (
    <div className="flex flex-col items-center justify-center py-12 text-center">
      <div className="text-4xl mb-3">{icon}</div>
      <h3 className="text-zinc-200 font-medium text-base mb-1">{title}</h3>
      {description && <p className="text-zinc-500 text-sm max-w-sm mb-4">{description}</p>}
      {action}
    </div>
  );
}
