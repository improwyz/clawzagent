import { useState } from 'react';

interface ChatInputProps {
  onSend?: (text: string) => void;
  disabled?: boolean;
  placeholder?: string;
}

export function ChatInput({ onSend, disabled, placeholder = 'Message...' }: ChatInputProps) {
  const [value, setValue] = useState('');

  const submit = () => {
    const text = value.trim();
    if (!text || disabled) return;
    setValue('');
    onSend?.(text);
  };

  return (
    <div className="flex items-center gap-2 bg-zinc-800 rounded-lg border border-zinc-700 focus-within:border-blue-500 transition-colors p-1">
      <input
        className="flex-1 bg-transparent text-zinc-100 text-sm px-2 py-1.5 focus:outline-none placeholder-zinc-500"
        placeholder={placeholder}
        value={value}
        disabled={disabled}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); submit(); } }}
      />
      <button
        onClick={submit}
        disabled={disabled || !value.trim()}
        className="p-1.5 rounded-md bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-40 transition-colors text-sm"
      >
        ↑
      </button>
    </div>
  );
}
