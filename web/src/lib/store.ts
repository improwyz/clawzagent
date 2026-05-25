import { create } from 'zustand';

interface AppStore {
  theme: 'dark' | 'light';
  toggleTheme: () => void;
  sidebarOpen: boolean;
  toggleSidebar: () => void;
  chatOpen: boolean;
  toggleChat: () => void;
  chatAgentId: string | null;
  setChatAgentId: (id: string | null) => void;
  notifications: Notification[];
  addNotification: (n: Omit<Notification, 'id'>) => void;
  dismissNotification: (id: string) => void;
}

interface Notification {
  id: string;
  type: 'info' | 'success' | 'warning' | 'error';
  message: string;
}

let notifId = 0;

export const useAppStore = create<AppStore>((set) => ({
  theme: 'dark',
  toggleTheme: () =>
    set((s) => {
      const next = s.theme === 'dark' ? 'light' : 'dark';
      document.documentElement.classList.toggle('dark', next === 'dark');
      return { theme: next };
    }),
  sidebarOpen: true,
  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
  chatOpen: true,
  toggleChat: () => set((s) => ({ chatOpen: !s.chatOpen })),
  chatAgentId: null,
  setChatAgentId: (id) => set({ chatAgentId: id }),
  notifications: [],
  addNotification: (n) =>
    set((s) => ({
      notifications: [...s.notifications, { ...n, id: String(++notifId) }],
    })),
  dismissNotification: (id) =>
    set((s) => ({ notifications: s.notifications.filter((n) => n.id !== id) })),
}));
